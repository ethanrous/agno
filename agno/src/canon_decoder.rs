//! Canon CR2 Lossless JPEG decoder
//!
//! CR2 files use LJPEG (Lossless JPEG) compression. This is standard JPEG
//! with SOF3 (lossless) frame type and Huffman-coded differential values.
//! Canon CR2 images are divided into vertical slices that must be stitched together.

use std::io::{Read, Seek, SeekFrom};

use tracing::debug;

use crate::sony_decoder::{DecodeError, Dimensions};

pub struct CanonLoadResult {
    pub pixels: Vec<u16>,
    pub white_level: u16,
}

/// Canon CR2 slice information
#[derive(Debug, Clone)]
pub struct Cr2SliceInfo {
    /// Width of each slice in pixels
    pub slice_widths: Vec<usize>,
}

impl Cr2SliceInfo {
    /// Parse CR2 slice info from raw tag data
    /// dcraw format: [count, slice_width, last_slice_width]
    /// total = count * slice_width + last_slice_width
    pub fn from_raw(data: &[u16], _total_width: usize) -> Self {
        if data.len() < 3 {
            // No valid slice info - this shouldn't happen for Canon CR2
            return Self {
                slice_widths: vec![_total_width],
            };
        }

        let count = data[0] as usize;
        let slice_width = data[1] as usize;
        let last_slice_width = data[2] as usize;

        // If count is 0, use slice_width as the count (dcraw behavior)
        let actual_count = if count == 0 { slice_width } else { count };

        let mut widths = Vec::with_capacity(actual_count + 1);

        // Add 'count' slices of slice_width
        for _ in 0..actual_count {
            widths.push(slice_width);
        }

        // Add the last slice if it has non-zero width
        if last_slice_width > 0 {
            widths.push(last_slice_width);
        }

        debug!(
            count = count,
            slice_width = slice_width,
            last_slice_width = last_slice_width,
            total_slices = widths.len(),
            total_width = widths.iter().sum::<usize>(),
            "CR2 slices"
        );

        Self {
            slice_widths: widths,
        }
    }

    /// Create default slice info (single slice = full width)
    pub fn single_slice(width: usize) -> Self {
        Self {
            slice_widths: vec![width],
        }
    }
}

/// Lossless JPEG bitstream reader for Canon CR2
struct LjpegBitstream<'a, R: Read> {
    reader: &'a mut R,
    bitbuf: u32,
    vbits: i32,
    restart_pending: bool,
}

impl<'a, R: Read> LjpegBitstream<'a, R> {
    fn new(reader: &'a mut R) -> Self {
        Self {
            reader,
            bitbuf: 0,
            vbits: 0,
            restart_pending: false,
        }
    }

    /// Reset the bit buffer (called at restart markers)
    fn reset(&mut self) {
        self.bitbuf = 0;
        self.vbits = 0;
        self.restart_pending = false;
    }

    /// Fill the bit buffer to have at least `need` bits
    fn fill(&mut self, need: i32) -> Result<(), DecodeError> {
        while self.vbits < need {
            let mut b = [0u8; 1];
            if self.reader.read(&mut b)? == 0 {
                break;
            }
            let c = b[0];

            // Handle JPEG byte stuffing: 0xFF followed by 0x00 is just 0xFF
            if c == 0xff {
                let mut next = [0u8; 1];
                if self.reader.read(&mut next)? == 0 {
                    break;
                }
                if next[0] == 0x00 {
                    // Byte stuffing - 0xFF00 means literal 0xFF
                    self.bitbuf = (self.bitbuf << 8) | 0xff;
                    self.vbits += 8;
                } else if next[0] >= 0xd0 && next[0] <= 0xd7 {
                    // Restart marker (RST0-RST7) - reset decoder state
                    self.restart_pending = true;
                    // Don't add the marker to bitbuf, just continue
                } else {
                    // Other marker - stop reading (end of scan)
                    break;
                }
                continue;
            }

            self.bitbuf = (self.bitbuf << 8) | (c as u32);
            self.vbits += 8;
        }
        Ok(())
    }

    /// Get n bits from the stream
    fn get_bits(&mut self, n: i32) -> Result<u32, DecodeError> {
        if n == 0 {
            return Ok(0);
        }
        self.fill(n)?;
        if self.vbits < n {
            return Err(DecodeError::CorruptData("Unexpected end of bitstream"));
        }
        self.vbits -= n;
        Ok((self.bitbuf >> self.vbits) & ((1 << n) - 1))
    }

    /// Decode a Huffman symbol using the given table
    fn decode_huffman(&mut self, huff: &HuffmanTable) -> Result<u8, DecodeError> {
        self.fill(16)?;

        // Peek at up to 16 bits
        let peek = if self.vbits >= 16 {
            (self.bitbuf >> (self.vbits - 16)) & 0xFFFF
        } else if self.vbits > 0 {
            (self.bitbuf << (16 - self.vbits)) & 0xFFFF
        } else {
            return Err(DecodeError::CorruptData("No bits available for Huffman"));
        };

        // Use lookup table for fast decoding
        let entry = huff.lookup[peek as usize];
        let len = (entry >> 8) as i32;
        let sym = (entry & 0xFF) as u8;

        if len == 0 || len > self.vbits {
            return Err(DecodeError::CorruptData("Invalid Huffman code"));
        }

        self.vbits -= len;
        Ok(sym)
    }

    /// Decode a LJPEG difference value
    fn decode_diff(&mut self, huff: &HuffmanTable) -> Result<i32, DecodeError> {
        let len = self.decode_huffman(huff)? as i32;

        if len == 0 {
            return Ok(0);
        }

        if len == 16 {
            return Ok(-32768);
        }

        let bits = self.get_bits(len)? as i32;

        // Sign extension: if the high bit is 0, it's negative
        let sign_bit = 1 << (len - 1);
        if (bits & sign_bit) == 0 {
            Ok(bits - (1 << len) + 1)
        } else {
            Ok(bits)
        }
    }
}

/// Huffman table for LJPEG decoding
struct HuffmanTable {
    /// Lookup table: index by 16-bit code prefix, value = (len << 8) | symbol
    lookup: Vec<u16>,
}

impl HuffmanTable {
    fn new() -> Self {
        Self {
            lookup: vec![0; 65536],
        }
    }

    /// Build lookup table from BITS and HUFFVAL arrays (DHT marker data)
    fn build(&mut self, bits: &[u8; 16], huffval: &[u8]) -> Result<(), DecodeError> {
        // Clear table
        self.lookup.fill(0);

        let mut code: u32 = 0;
        let mut val_idx = 0;

        for (bit_len, &count) in bits.iter().enumerate() {
            let bit_len = bit_len + 1; // 1-based
            for _ in 0..count {
                if val_idx >= huffval.len() {
                    return Err(DecodeError::CorruptData("Huffman table overflow"));
                }
                let sym = huffval[val_idx];
                val_idx += 1;

                // Fill lookup table entries for this code
                // The code occupies `bit_len` bits, pad with all possible suffixes
                let shift = 16 - bit_len;
                let fill_count = 1 << shift;
                let entry = ((bit_len as u16) << 8) | (sym as u16);

                for i in 0..fill_count {
                    let index = ((code << shift) | i) as usize;
                    self.lookup[index] = entry;
                }

                code += 1;
            }
            code <<= 1;
        }

        Ok(())
    }
}

/// Parse LJPEG markers and decode the image
pub fn canon_ljpeg_load_raw<R: Read + Seek>(
    reader: &mut R,
    dims: Dimensions,
    slice_info: Option<Cr2SliceInfo>,
) -> Result<CanonLoadResult, DecodeError> {
    // Canon CR2 LJPEG structure:
    // SOI (0xFFD8)
    // DHT markers (0xFFC4) - Huffman tables
    // SOF3 (0xFFC3) - Lossless JPEG frame header
    // SOS (0xFFDA) - Scan header, followed by compressed data

    let mut huff_tables: [Option<HuffmanTable>; 4] = [None, None, None, None];
    let mut precision = 14u8;
    let mut _ljpeg_width = dims.output_width as u16;
    let mut _ljpeg_height = dims.output_height as u16;
    let mut components = 1u8;
    let mut comp_info: Vec<(u8, u8)> = Vec::new(); // (h_samp, v_samp) for each component

    // Parse markers
    loop {
        let marker = read_marker(reader)?;

        match marker {
            0xD8 => {
                // SOI - Start of Image, continue
            }
            0xC4 => {
                // DHT - Define Huffman Table
                let length = read_be_u16(reader)? as usize - 2;
                let mut data = crate::guard::try_vec(0u8, length)?;
                reader.read_exact(&mut data)?;

                let mut pos = 0;
                while pos < data.len() {
                    let tc_th = data[pos];
                    let tc = (tc_th >> 4) & 0x0F; // Table class (0=DC, 1=AC)
                    let th = tc_th & 0x0F; // Table destination

                    if tc > 1 || th > 3 {
                        return Err(DecodeError::CorruptData("Invalid DHT table class/id"));
                    }

                    pos += 1;

                    // Read BITS (16 bytes)
                    let mut bits = [0u8; 16];
                    bits.copy_from_slice(&data[pos..pos + 16]);
                    pos += 16;

                    // Count total symbols
                    let total: usize = bits.iter().map(|&x| x as usize).sum();

                    // Read HUFFVAL
                    let huffval: Vec<u8> = data[pos..pos + total].to_vec();
                    pos += total;

                    // Build table (for LJPEG, we only use DC tables, but Canon may define multiple)
                    let table_idx = (tc * 2 + th) as usize;
                    let mut table = HuffmanTable::new();
                    table.build(&bits, &huffval)?;

                    huff_tables[table_idx] = Some(table);
                }
            }
            0xC3 => {
                // SOF3 - Lossless JPEG frame header
                let _length = read_be_u16(reader)?;
                precision = read_u8(reader)?;
                _ljpeg_height = read_be_u16(reader)?;
                _ljpeg_width = read_be_u16(reader)?;
                components = read_u8(reader)?;

                comp_info.clear();
                for _ in 0..components {
                    let _comp_id = read_u8(reader)?;
                    let sampling = read_u8(reader)?;
                    let h_samp = (sampling >> 4) & 0x0F;
                    let v_samp = sampling & 0x0F;
                    let _quant_table = read_u8(reader)?;
                    comp_info.push((h_samp, v_samp));
                }

                debug!(
                    precision = precision,
                    ljpeg_width = _ljpeg_width,
                    ljpeg_height = _ljpeg_height,
                    components = components,
                    "LJPEG SOF3 header"
                );
            }
            0xDA => {
                // SOS - Start of Scan
                let length = read_be_u16(reader)? as usize - 2;
                let mut sos_data = crate::guard::try_vec(0u8, length)?;
                reader.read_exact(&mut sos_data)?;

                let ns = sos_data[0] as usize; // Number of components in scan

                // Read component selectors and table mappings
                let mut comp_tables: Vec<usize> = Vec::new();
                for i in 0..ns {
                    let _cs = sos_data[1 + i * 2]; // Component selector
                    let td_ta = sos_data[2 + i * 2];
                    let td = ((td_ta >> 4) & 0x0F) as usize; // DC table selector
                    comp_tables.push(td);
                }

                // Read predictor selection and point transform
                let _predictor = sos_data[1 + ns * 2];
                let _se = sos_data[2 + ns * 2]; // End of spectral selection
                let ah_al = sos_data[3 + ns * 2];
                let _point_transform = ah_al & 0x0F;

                // Now decode the entropy-coded segment
                return decode_ljpeg_data(
                    reader,
                    dims,
                    components,
                    precision,
                    &huff_tables,
                    &comp_tables,
                    slice_info,
                );
            }
            0xD9 => {
                // EOI - End of Image
                break;
            }
            0xFF => {
                // Fill bytes, skip
            }
            _ => {
                // Unknown marker, skip its data
                if marker >= 0xC0 {
                    let length = read_be_u16(reader)? as i64 - 2;
                    if length > 0 {
                        reader.seek(SeekFrom::Current(length))?;
                    }
                }
            }
        }
    }

    Err(DecodeError::CorruptData("No SOS marker found in LJPEG"))
}

fn decode_ljpeg_data<R: Read>(
    reader: &mut R,
    dims: Dimensions,
    components: u8,
    precision: u8,
    huff_tables: &[Option<HuffmanTable>; 4],
    comp_tables: &[usize],
    slice_info: Option<Cr2SliceInfo>,
) -> Result<CanonLoadResult, DecodeError> {
    crate::guard::check_dims(dims.raw_width as u64, dims.raw_height as u64)?;

    let mut bs = LjpegBitstream::new(reader);

    let mut pixels = crate::guard::try_vec(0u16, dims.raw_width * dims.raw_height)?;

    // Initial prediction value: 2^(precision-1) is standard for LJPEG
    let initial_pred = 1 << (precision - 1);

    // Canon CR2 typically has 2 or 4 components interleaved
    let num_comps = components as usize;

    // Get the Huffman table for each component
    let get_table = |comp_idx: usize| -> Result<&HuffmanTable, DecodeError> {
        let table_idx = if comp_idx < comp_tables.len() {
            comp_tables[comp_idx]
        } else {
            0
        };
        huff_tables[table_idx]
            .as_ref()
            .ok_or(DecodeError::CorruptData("Missing Huffman table"))
    };

    // Get slice widths - if we have slice info, use it; otherwise single slice
    let slices = slice_info.unwrap_or_else(|| Cr2SliceInfo::single_slice(dims.raw_width));

    debug!(
        raw_width = dims.raw_width,
        raw_height = dims.raw_height,
        components = num_comps,
        slices = ?slices.slice_widths,
        "Decoding LJPEG"
    );

    let jh_high = dims.output_height;

    // Canon CR2 LJPEG decoding following dcraw algorithm exactly
    //
    // The LJPEG encodes data in vertical slices. Each slice covers the full height
    // but only part of the width. Samples are arranged slice-first:
    // - First: all samples for slice 0 (cols 0 to slice_width-1, all rows)
    // - Then: all samples for slice 1 (cols slice_width to 2*slice_width-1, all rows)
    //
    // Each LJPEG row contains (slice_width) samples that fill multiple output rows
    // of the current slice.

    // dcraw slice format: cr2_slice[0]=count, cr2_slice[1]=width, cr2_slice[2]=last_width
    // Our slices.slice_widths = [2784, 2784] for a 2-slice image
    // So: count=1 (number of middle slices), width=2784, last_width=2784
    let cr2_slice_0 = slices.slice_widths.len() - 1; // 1 (count of "middle" slices)
    let cr2_slice_1 = slices.slice_widths[0]; // 2784 (width of middle slices)
    let cr2_slice_2 = *slices.slice_widths.last().unwrap(); // 2784 (width of last slice)

    // Canon CR2 LJPEG: jwide is the number of samples per LJPEG row
    // For 2-component LJPEG with width W, there are W*2 samples per row
    // The LJPEG width from SOF3 is stored as cr2_slice_1 (slice width)
    // Total samples per row = raw_width = slice_width * num_slices * num_components...
    // Actually, for Canon: jwide = LJPEG_width * num_components = slice_width * components
    let ljpeg_width = cr2_slice_1; // LJPEG width from SOF3
    let jwide = ljpeg_width * num_comps;

    debug!(
        jwide = jwide,
        jh_high = jh_high,
        cr2_slice_0 = cr2_slice_0,
        cr2_slice_1 = cr2_slice_1,
        cr2_slice_2 = cr2_slice_2,
        total_samples = jwide * jh_high,
        "LJPEG decode parameters"
    );

    // Canon CR2 LJPEG decoding with proper predictor handling
    //
    // Per LJPEG spec (ITU-T.81), the predictor for the first sample of each row
    // depends on the row number:
    // - Row 0, Col 0: predictor = 2^(precision-1) (initial value)
    // - Row 0, Col >0: predictor = Ra (left pixel)
    // - Row >0, Col 0: predictor = Rb (top pixel) - CRITICAL: use row above, not initial!
    // - Row >0, Col >0: predictor = Ra (left pixel, for predictor mode 1)
    //
    // We need to store the first value of each component from the previous row
    // to use as the predictor for the first column of subsequent rows.

    let mut pred = vec![initial_pred; num_comps];
    let mut row_start_pred = vec![initial_pred; num_comps]; // First value of each component in prev row
    let samples_per_slice = cr2_slice_1 * jh_high;

    for jrow in 0..jh_high {
        // At the start of each LJPEG row, reset predictor to the first sample of the previous row
        // (for row 0, this is the initial value; for subsequent rows, it's the first decoded value)
        for (i, p) in pred.iter_mut().enumerate() {
            *p = row_start_pred[i];
        }

        // We'll capture the first decoded value of each component to use for the next row
        let mut first_values = vec![0i32; num_comps];
        let mut got_first = vec![false; num_comps];

        // Decode one LJPEG row: components interleaved
        for jcol in 0..jwide {
            // Check for restart marker (resets decoder state)
            if bs.restart_pending {
                for p in pred.iter_mut() {
                    *p = initial_pred;
                }
                bs.reset();
            }

            // Calculate output position using dcraw-style slice mapping
            let jidx = jrow * jwide + jcol;
            let mut slice_idx = jidx / samples_per_slice;
            let is_last_slice = slice_idx >= cr2_slice_0;
            if is_last_slice {
                slice_idx = cr2_slice_0;
            }

            let c = jcol % num_comps;
            let table = get_table(c)?;
            let diff = bs.decode_diff(table)?;

            let value = (pred[c] + diff).clamp(0, (1 << precision) - 1) as u16;
            pred[c] = value as i32;

            // Capture the first value of each component for next row's predictor
            if !got_first[c] {
                first_values[c] = value as i32;
                got_first[c] = true;
            }

            // Complete the output position calculation
            let jidx_in_slice = jidx - slice_idx * samples_per_slice;
            let slice_width = if is_last_slice {
                cr2_slice_2
            } else {
                cr2_slice_1
            };
            let row = jidx_in_slice / slice_width;
            let col = jidx_in_slice % slice_width + slice_idx * cr2_slice_1;

            if col < dims.raw_width && row < dims.raw_height {
                let idx = row * dims.raw_width + col;
                if idx < pixels.len() {
                    pixels[idx] = value;
                }
            }
        }

        // Update row_start_pred with the first values from this row
        for (i, &val) in first_values.iter().enumerate() {
            if got_first[i] {
                row_start_pred[i] = val;
            }
        }
    }

    // White level based on precision (typically 14-bit = 16383)
    let white_level = ((1u32 << precision) - 1) as u16;

    // Debug: write raw pixels to file for inspection
    if std::env::var("DEBUG_RAW").is_ok() {
        use std::io::Write;
        let mut f = std::fs::File::create("/tmp/debug_raw.gray").unwrap();
        for p in &pixels {
            // Scale 14-bit to 16-bit for viewing
            let scaled = ((*p as u32) << 2) as u16;
            f.write_all(&scaled.to_le_bytes()).unwrap();
        }
        tracing::warn!(
            raw_width = dims.raw_width,
            raw_height = dims.raw_height,
            path = "/tmp/debug_raw.gray",
            "Wrote debug raw (16-bit gray)"
        );
    }

    Ok(CanonLoadResult {
        pixels,
        white_level,
    })
}

/// Load uncompressed Canon RAW data
pub fn canon_uncompressed_load_raw<R: Read>(
    reader: &mut R,
    dims: Dimensions,
    bits_per_sample: u16,
) -> Result<CanonLoadResult, DecodeError> {
    crate::guard::check_dims(dims.raw_width as u64, dims.raw_height as u64)?;

    let mut pixels = crate::guard::try_vec(0u16, dims.raw_width * dims.raw_height)?;

    // Read 16-bit little-endian values
    let mut row_buf = crate::guard::try_vec(0u8, dims.output_width * 2)?;

    for y in 0..dims.output_height {
        reader.read_exact(&mut row_buf)?;
        for x in 0..dims.output_width {
            let lo = row_buf[2 * x];
            let hi = row_buf[2 * x + 1];
            let v = u16::from_le_bytes([lo, hi]);
            pixels[y * dims.raw_width + x] = v;
        }
    }

    let white_level = ((1u32 << bits_per_sample) - 1) as u16;

    Ok(CanonLoadResult {
        pixels,
        white_level,
    })
}

// Helper functions for reading JPEG markers

fn read_marker<R: Read>(reader: &mut R) -> Result<u8, DecodeError> {
    let mut b = [0u8; 1];

    // Find 0xFF
    loop {
        if reader.read(&mut b)? == 0 {
            return Err(DecodeError::CorruptData(
                "Unexpected EOF looking for marker",
            ));
        }
        if b[0] == 0xFF {
            break;
        }
    }

    // Read marker code, skipping fill bytes
    loop {
        if reader.read(&mut b)? == 0 {
            return Err(DecodeError::CorruptData("Unexpected EOF in marker"));
        }
        if b[0] != 0xFF {
            return Ok(b[0]);
        }
    }
}

fn read_u8<R: Read>(reader: &mut R) -> Result<u8, DecodeError> {
    let mut b = [0u8; 1];
    reader.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_be_u16<R: Read>(reader: &mut R) -> Result<u16, DecodeError> {
    let mut b = [0u8; 2];
    reader.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    // Absurd Dimensions (header-declared): raw_width * raw_height overflows the guard's
    // MAX_PIXELS budget, so decode functions must reject via `check_dims` before any
    // multi-terabyte allocation is attempted.
    fn absurd_dims() -> Dimensions {
        Dimensions {
            raw_width: 1 << 20,
            raw_height: 1 << 20,
            output_width: 1 << 20,
            output_height: 1 << 20,
        }
    }

    #[test]
    fn canon_uncompressed_load_raw_rejects_absurd_dimensions() {
        let mut cur = Cursor::new(Vec::<u8>::new());
        let result = canon_uncompressed_load_raw(&mut cur, absurd_dims(), 14);
        assert!(result.is_err(), "expected absurd dimensions to be rejected");
    }

    #[test]
    fn decode_ljpeg_data_rejects_absurd_dimensions() {
        let huff_tables: [Option<HuffmanTable>; 4] = [None, None, None, None];
        let mut cur = Cursor::new(Vec::<u8>::new());
        let result = decode_ljpeg_data(&mut cur, absurd_dims(), 1, 14, &huff_tables, &[], None);
        assert!(result.is_err(), "expected absurd dimensions to be rejected");
    }
}
