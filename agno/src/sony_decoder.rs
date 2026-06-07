use std::io::{Read, Seek, SeekFrom};

use crate::tiff::SonyVariant;

#[derive(Debug)]
pub enum DecodeError {
    Io(std::io::Error),
    CorruptData(&'static str),
    UnsupportedFormat(SonyVariant),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Io(e) => write!(f, "I/O error: {}", e),
            DecodeError::CorruptData(msg) => write!(f, "Corrupt data: {}", msg),
            DecodeError::UnsupportedFormat(v) => {
                write!(f, "Unsupported Sony RAW format: {:?}", v)
            }
        }
    }
}

impl From<std::io::Error> for DecodeError {
    fn from(err: std::io::Error) -> DecodeError {
        DecodeError::Io(err)
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Dimensions {
    // Full raw raster size (stride and height for destination buffer)
    pub raw_width: usize,
    pub raw_height: usize,
    // Active image area (what decoders actually write)
    pub output_width: usize,
    pub output_height: usize,
}

pub struct SonyLoadResult {
    pub pixels: Vec<u16>, // row-major, size = raw_width * raw_height
    pub white_level: u16, // LibRaw’s “maximum”
}

// ====================== Utilities ======================

// fn seek_set<S: Seek>(s: &mut S, offset: u64) -> Result<(), DecodeError> {
//     s.seek(SeekFrom::Start(offset))?;
//     Ok(())
// }

// fn seek_cur<S: Seek>(s: &mut S, delta: i64) -> Result<(), DecodeError> {
//     s.seek(SeekFrom::Current(delta))?;
//     Ok(())
// }

// fn read_u8<R: Read>(r: &mut R) -> Result<u8, DecodeError> {
//     let mut b = [0u8; 1];
//     r.read_exact(&mut b)?;
//     Ok(b[0])
// }

// fn read_u32_be<R: Read>(r: &mut R) -> Result<u32, DecodeError> {
//     let mut b = [0u8; 4];
//     r.read_exact(&mut b)?;
//     Ok(u32::from_be_bytes(b))
// }

// #[inline]
// fn ntohs_be(bytes: [u8; 2]) -> u16 {
//     u16::from_be_bytes(bytes)
// }

// // Interpret a byte slice as little-endian u32 words (b0=LSB) like a native cast on LE platforms.
// // We materialize words to match the C code’s cast-and-XOR semantics (with htonl used on the pad).
// fn as_le_u32_words(buf: &[u8]) -> Vec<u32> {
//     let mut words = Vec::with_capacity(buf.len() / 4);
//     for chunk in buf.chunks_exact(4) {
//         words.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
//     }
//     words
// }

// fn write_le_u32_words(words: &[u32], buf: &mut [u8]) {
//     for (i, chunk) in buf.chunks_exact_mut(4).enumerate() {
//         chunk.copy_from_slice(&words[i].to_le_bytes());
//     }
// }

// ====================== Bitstream (getbits/getbithuff/ljpeg_diff) ======================

pub struct JpegBitstream<'a, R: Read> {
    reader: &'a mut R,
    bitbuf: u32,
    vbits: i32,
    reset: bool,
    zero_after_ff: bool,
    pub dng_version: Option<u32>,
}

impl<'a, R: Read> JpegBitstream<'a, R> {
    pub fn new(reader: &'a mut R) -> Self {
        Self {
            reader,
            bitbuf: 0,
            vbits: 0,
            reset: false,
            zero_after_ff: false,
            dng_version: None,
        }
    }

    pub fn set_zero_after_ff(&mut self, enabled: bool) {
        self.zero_after_ff = enabled;
    }

    // getbits(-1) equivalent
    pub fn reset_state(&mut self) {
        self.bitbuf = 0;
        self.vbits = 0;
        self.reset = false;
    }

    fn fill_to(&mut self, need: i32) -> Result<(), DecodeError> {
        if need > 25 || need <= 0 || self.vbits < 0 {
            return Ok(());
        }
        while !self.reset && self.vbits < need {
            let mut b = [0u8; 1];
            let n = self.reader.read(&mut b)?;
            if n == 0 {
                break;
            }
            let c = b[0];
            if self.zero_after_ff && c == 0xff {
                let mut next = [0u8; 1];
                let n2 = self.reader.read(&mut next)?;
                if n2 == 0 {
                    self.reset = true;
                    break;
                }
                if next[0] != 0 {
                    self.reset = true;
                    break;
                }
                // stuffed zero -> accept 0xff as data
                self.bitbuf = (self.bitbuf << 8) | (c as u32);
                self.vbits += 8;
                continue;
            }
            self.bitbuf = (self.bitbuf << 8) | (c as u32);
            self.vbits += 8;
        }
        Ok(())
    }

    fn getbithuff(&mut self, nbits: i32, huff: Option<&[u16]>) -> Result<u32, DecodeError> {
        if nbits > 25 {
            return Ok(0);
        }
        if nbits < 0 {
            self.reset_state();
            return Ok(0);
        }
        if nbits == 0 || self.vbits < 0 {
            return Ok(0);
        }
        self.fill_to(nbits)?;
        let c = if self.vbits == 0 {
            0
        } else {
            let shift = 32 - self.vbits;
            (self.bitbuf << shift) >> (32 - nbits)
        };
        if let Some(table) = huff {
            let entry = table[c as usize];
            let code_len = (entry >> 8) as i32;
            let sym = (entry & 0xff) as u32;
            self.vbits -= code_len;
            if self.vbits < 0 {
                return Err(DecodeError::CorruptData("getbithuff(huff) underflow"));
            }
            Ok(sym)
        } else {
            self.vbits -= nbits;
            if self.vbits < 0 {
                return Err(DecodeError::CorruptData("getbithuff(bits) underflow"));
            }
            Ok(c)
        }
    }

    fn gethuff(&mut self, huff: &[u16]) -> Result<i32, DecodeError> {
        Ok(self.getbithuff(15, Some(huff))? as i32)
    }

    // Port of ljpeg_diff using the provided Huffman table
    pub fn ljpeg_diff(&mut self, huff: &[u16]) -> Result<i32, DecodeError> {
        let len = self.gethuff(huff)?;
        if len == 16 {
            let dv = self.dng_version.unwrap_or(0);
            if dv == 0 || dv >= 0x1010000 {
                return Ok(-32768);
            }
        }
        let bits = if len > 0 {
            self.getbithuff(len, None)?
        } else {
            0
        };
        let mut diff = bits as i32;
        if len > 0 {
            let sign_bit = 1i32 << (len - 1);
            if (diff & sign_bit) == 0 {
                diff -= (1i32 << len) - 1;
            }
        }
        Ok(diff)
    }
}

// ====================== Sony decrypt (ported) ======================

/// Trait for Sony decryption operations
pub trait SonyDecrypt {
    fn decrypt_u32_words(&mut self, data_32bit_words: &mut [u32], start_sequence: bool, key: u32);
}

/// Direct port of LibRaw::sony_decrypt working on u32 words (LE-packed).
pub struct SonyDecryptor {
    pad: [u32; 128],
    p: u32,
}

impl SonyDecryptor {
    pub fn new() -> Self {
        Self {
            pad: [0; 128],
            p: 0,
        }
    }

    fn init_pad(&mut self, key: u32) {
        let mut k = key as u64;
        for i in 0..4 {
            k = k.wrapping_mul(48_828_125u64).wrapping_add(1);
            self.pad[i] = k as u32;
        }
        // pad[3] = pad[3] << 1 | (pad[0] ^ pad[2]) >> 31;
        self.pad[3] = (self.pad[3] << 1) | (((self.pad[0] ^ self.pad[2]) >> 31) & 1);

        for i in 4..127 {
            // (pad[p-4] ^ pad[p-2]) << 1 | (pad[p-3] ^ pad[p-1]) >> 31
            let left = (self.pad[i - 4] ^ self.pad[i - 2]) << 1;
            let right = ((self.pad[i - 3] ^ self.pad[i - 1]) >> 31) & 1;
            self.pad[i] = left | right;
        }
        // htonl equivalent
        for i in 0..127 {
            self.pad[i] = self.pad[i].to_be();
        }
        // In dcraw, p ends at 127 after the htonl loop (for p=0; p<127; p++)
        // The decrypt loop then starts with p++, making it 128
        self.p = 127;
    }
}

impl Default for SonyDecryptor {
    fn default() -> Self {
        Self::new()
    }
}

impl SonyDecrypt for SonyDecryptor {
    fn decrypt_u32_words(&mut self, data_32bit_words: &mut [u32], start_sequence: bool, key: u32) {
        if start_sequence {
            self.init_pad(key);
        }
        for w in data_32bit_words.iter_mut() {
            // Advance pad stream: pad[p&127] = pad[(p+1)&127] ^ pad[(p+65)&127]
            let idx = (self.p & 127) as usize;
            let i1 = ((self.p + 1) & 127) as usize;
            let i2 = ((self.p + 65) & 127) as usize;
            self.pad[idx] = self.pad[i1] ^ self.pad[i2];
            *w ^= self.pad[idx];
            self.p = self.p.wrapping_add(1);
        }
    }
}

/// Decrypt a byte buffer in-place using the Sony cipher.
/// The buffer length must be a multiple of 4 bytes.
pub fn decrypt_sr2_data(decryptor: &mut SonyDecryptor, data: &mut [u8], key: u32) {
    // Convert bytes to u32 words (little-endian)
    let word_count = data.len() / 4;
    let mut words = Vec::with_capacity(word_count);
    for chunk in data.chunks_exact(4) {
        words.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }

    // Decrypt
    decryptor.decrypt_u32_words(&mut words, true, key);

    // Write back to bytes (little-endian)
    for (i, chunk) in data.chunks_exact_mut(4).enumerate() {
        chunk.copy_from_slice(&words[i].to_le_bytes());
    }
}

// ====================== Decoders ======================

// Port of LibRaw::sony_load_raw
// pub fn sony_load_raw<R: Read + Seek>(
//     reader: &mut R,
//     dims: Dimensions,
//     data_offset: u64,
//     decryptor: &mut dyn SonyDecrypt,
// ) -> Result<SonyLoadResult, DecodeError> {
//     // Seek and compute initial key
//     seek_set(reader, 200_896)?;
//     let step_byte = read_u8(reader)? as u64;
//     let step = step_byte.saturating_mul(4).saturating_sub(1);
//     seek_cur(reader, step as i64)?;
//     let mut key = read_u32_be(reader)?;
//
//     // Secondary header: decrypt and extend key using bytes 26..=23
//     seek_set(reader, 164_600)?;
//     let mut header_block = [0u8; 40];
//     reader.read_exact(&mut header_block)?;
//     {
//         let mut words = as_le_u32_words(&header_block);
//         decryptor.decrypt_u32_words(&mut words[..10], /*start_sequence*/ true, key);
//         write_le_u32_words(&words, &mut header_block);
//     }
//     for i in (23..=26).rev() {
//         key = (key << 8) | (header_block[i] as u32);
//     }
//
//     // Read and decode rows
//     let mut pixels = vec![0u16; dims.raw_width * dims.raw_height];
//     seek_set(reader, data_offset)?;
//     let mut row_bytes = vec![0u8; dims.raw_width * 2];
//
//     for row in 0..dims.raw_height {
//         reader.read_exact(&mut row_bytes)?;
//         // Decrypt this row as u32 words (raw_width/2 words)
//         {
//             let mut words = as_le_u32_words(&row_bytes);
//             let start_sequence = (row % 2) == 0; // C code passes !row (true for row==0)
//             decryptor.decrypt_u32_words(&mut words, start_sequence, key);
//             write_le_u32_words(&words, &mut row_bytes);
//         }
//         // ntohs and top-bit check
//         for col in 0..dims.raw_width {
//             let v = ntohs_be([row_bytes[2 * col], row_bytes[2 * col + 1]]);
//             if (v >> 14) != 0 {
//                 return Err(DecodeError::CorruptData("Sony: invalid top bits in pixel"));
//             }
//             pixels[row * dims.raw_width + col] = v;
//         }
//     }
//
//     Ok(SonyLoadResult {
//         pixels,
//         white_level: 0x3ff0,
//     })
// }

// Port of LibRaw::sony_arw_load_raw (LJPEG-style differential decoding)
// pub fn sony_arw_load_raw<R: Read + Seek>(
//     reader: &mut R,
//     dims: Dimensions,
//     zero_after_ff: bool,
//     dng_version: Option<u32>,
// ) -> Result<SonyLoadResult, DecodeError> {
//     // Build fixed Huffman table
//     const TAB: [u16; 18] = [
//         0x0f11, 0x0f10, 0x0e0f, 0x0d0e, 0x0c0d, 0x0b0c, 0x0a0b, 0x090a, 0x0809, 0x0708, 0x0607,
//         0x0506, 0x0405, 0x0304, 0x0303, 0x0300, 0x0202, 0x0201,
//     ];
//     let mut huff = vec![0u16; 32770];
//     huff[0] = 15;
//     let mut n = 0usize;
//     for &entry in &TAB {
//         let cnt = 32768usize >> (entry >> 8);
//         for _ in 0..cnt {
//             n += 1;
//             huff[n] = entry;
//         }
//     }
//
//     // getbits(-1) reset
//     let mut bs = JpegBitstream::new(reader);
//     bs.set_zero_after_ff(zero_after_ff);
//     bs.dng_version = dng_version;
//     bs.reset_state();
//
//     let mut pixels = vec![0u16; dims.raw_width * dims.raw_height];
//     let mut acc: i32 = 0;
//
//     // Decode column-major, right-to-left
//     for col in (0..dims.raw_width).rev() {
//         let mut row = 0usize;
//         while row <= dims.raw_height {
//             if row == dims.raw_height {
//                 row = 1;
//             }
//             let diff = bs.ljpeg_diff(&huff)?;
//             acc += diff;
//             if (acc >> 12) != 0 {
//                 return Err(DecodeError::CorruptData("Sony ARW: accumulator overflow"));
//             }
//             if row < dims.output_height {
//                 pixels[row * dims.raw_width + col] = acc as u16;
//             }
//             row += 2;
//         }
//     }
//
//     Ok(SonyLoadResult {
//         pixels,
//         white_level: 0x0fff,
//     })
// }

/// Build the Sony ARW2 linearization (tone) curve from the `SonyToneCurve` tag (0x7010).
///
/// Port of dcraw/LibRaw: the four control points are reduced with `>> 2 & 0xfff` and
/// bracketed by 0 and 4095; within segment `i` the curve increments by `1 << i`. During
/// ARW2 decoding the curve is indexed by `pixel << 1`, expanding the 11-bit codes to the
/// ~14-bit linear domain (max ~`0x3ff0`). Returns a 0x4000-entry table; only indices
/// 0..=4094 are ever read (pixel codes are clamped to 0x7ff).
pub fn build_sony_tone_curve(points: [u16; 4]) -> Vec<u16> {
    let mut curve = vec![0u16; 0x4000];

    // Missing/zero tag: identity passthrough so ARW2 still decodes (no expansion).
    if points == [0, 0, 0, 0] {
        for (i, c) in curve.iter_mut().enumerate() {
            *c = i.min(0x3fff) as u16;
        }
        return curve;
    }

    let mut sc = [0usize; 6];
    sc[0] = 0;
    sc[5] = 4095;
    for i in 0..4 {
        sc[i + 1] = ((points[i] >> 2) & 0xfff) as usize;
    }

    for i in 0..5 {
        // Segments may be empty if the control points are non-monotonic; that's fine.
        for j in (sc[i] + 1)..=sc[i + 1] {
            curve[j] = curve[j - 1].saturating_add(1u16 << i);
        }
    }
    curve
}

// Port of LibRaw::sony_arw2_load_raw (block-based: 16 bytes -> 16 pixels).
// Each 16-byte block decodes 16 pixels that are written to every OTHER column (stride 2);
// consecutive blocks alternate between the even and odd column phase of a 32-column span.
// The 11-bit codes are expanded through the Sony tone curve (`curve[pix << 1]`) into the
// ~14-bit linear domain. Skipping the de-interleave produces a vertical comb artifact;
// skipping the curve makes the image ~8x too dark.
// `tone_curve` must have at least 0x1000 entries (as built by `build_sony_tone_curve`);
// pixel codes are clamped to 0x7ff, so the largest index read is `0x7ff << 1` = 0xffe.
#[allow(clippy::needless_range_loop)]
pub fn sony_arw2_load_raw<R: Read>(
    reader: &mut R,
    dims: Dimensions,
    tone_curve: &[u16],
) -> Result<SonyLoadResult, DecodeError> {
    let raw_width = dims.raw_width;
    let mut pixels = vec![0u16; dims.raw_width * dims.raw_height];

    // dcraw allocates raw_width + 1 so the 16-bit reads inside a block can over-read by one byte.
    let mut row_buf = vec![0u8; raw_width + 1];

    for row in 0..dims.output_height {
        reader.read_exact(&mut row_buf[..raw_width])?;
        row_buf[raw_width] = 0;

        let mut dp = 0usize;
        let mut col: usize = 0;

        while col < raw_width.saturating_sub(30) {
            if dp + 16 > raw_width {
                break;
            }

            let header = u32::from_le_bytes([
                row_buf[dp],
                row_buf[dp + 1],
                row_buf[dp + 2],
                row_buf[dp + 3],
            ]);

            let max_v = (header & 0x7ff) as i32;
            let min_v = ((header >> 11) & 0x7ff) as i32;
            let imax = ((header >> 22) & 0x0f) as usize;
            let imin = ((header >> 26) & 0x0f) as usize;

            let mut sh = 0;
            while sh < 4 && (0x80i32 << sh) <= (max_v - min_v) {
                sh += 1;
            }

            let mut pix16 = [0u16; 16];
            let mut bit = 30usize;
            for i in 0..16usize {
                if i == imax {
                    pix16[i] = max_v as u16;
                } else if i == imin {
                    pix16[i] = min_v as u16;
                } else {
                    let byte_index = dp + (bit >> 3);
                    if byte_index + 1 >= row_buf.len() {
                        return Err(DecodeError::CorruptData("Sony ARW2: row buffer overread"));
                    }
                    let two =
                        u16::from_le_bytes([row_buf[byte_index], row_buf[byte_index + 1]]) as i32;
                    let code7 = (two >> (bit & 7)) & 0x7f;
                    let mut value = (code7 << sh) + min_v;
                    if value > 0x7ff {
                        value = 0x7ff;
                    }
                    pix16[i] = value as u16;
                    bit += 7;
                }
            }

            // De-interleaved write with tone-curve expansion (curve indexed by pix << 1).
            let mut c = col;
            for i in 0..16usize {
                if c < dims.output_width {
                    pixels[row * dims.raw_width + c] = tone_curve[(pix16[i] as usize) << 1];
                }
                c += 2;
            }
            col = c - if c & 1 == 1 { 1 } else { 31 };
            dp += 16;
        }
    }

    Ok(SonyLoadResult {
        pixels,
        white_level: 0x3ff0,
    })
}

// Legacy ARW (LJPEG-like). Reads the full compressed bitstream from reader.
pub fn sony_arw_load_raw_from_stream<R: Read>(
    reader: &mut R,
    dims: Dimensions,
    zero_after_ff: bool,
    dng_version: Option<u32>,
) -> Result<SonyLoadResult, DecodeError> {
    let mut bs = JpegBitstream::new(reader);
    bs.set_zero_after_ff(zero_after_ff);
    bs.dng_version = dng_version;
    bs.reset_state();

    // Build Huffman table (fixed)
    const TAB: [u16; 18] = [
        0x0f11, 0x0f10, 0x0e0f, 0x0d0e, 0x0c0d, 0x0b0c, 0x0a0b, 0x090a, 0x0809, 0x0708, 0x0607,
        0x0506, 0x0405, 0x0304, 0x0303, 0x0300, 0x0202, 0x0201,
    ];
    let mut huff = vec![0u16; 32770];
    huff[0] = 15;
    let mut n = 0usize;
    for &entry in &TAB {
        let cnt = 32768usize >> (entry >> 8);
        for _ in 0..cnt {
            n += 1;
            huff[n] = entry;
        }
    }

    let mut pixels = vec![0u16; dims.raw_width * dims.raw_height];
    let mut acc: i32 = 0;

    for col in (0..dims.raw_width).rev() {
        let mut row = 0usize;
        while row <= dims.raw_height {
            if row == dims.raw_height {
                row = 1;
            }
            let diff = bs.ljpeg_diff(&huff)?;
            acc += diff;
            if (acc >> 12) != 0 {
                return Err(DecodeError::CorruptData("Sony ARW: accumulator overflow"));
            }
            if row < dims.output_height {
                pixels[row * dims.raw_width + col] = acc as u16;
            }
            row += 2;
        }
    }

    Ok(SonyLoadResult {
        pixels,
        white_level: 0x0fff,
    })
}

// 14-bit uncompressed: read 16-bit little-endian words, mask/check top bits if desired
pub fn sony_uncompressed14_load_raw<R: Read>(
    reader: &mut R,
    dims: Dimensions,
) -> Result<SonyLoadResult, DecodeError> {
    let mut pixels = vec![0u16; dims.raw_width * dims.raw_height];
    let mut row = vec![0u8; dims.output_width * 2];

    for y in 0..dims.output_height {
        reader.read_exact(&mut row)?;
        for x in 0..dims.output_width {
            let lo = row[2 * x] as u8;
            let hi = row[2 * x + 1] as u8;
            let v = u16::from_le_bytes([lo, hi]);
            pixels[y * dims.raw_width + x] = v; // 14-bit data in 16-bit container
        }
    }

    Ok(SonyLoadResult {
        pixels,
        white_level: 0x3fff,
    })
}

// Helper: read all strips and concatenate into a single buffer
pub fn read_concatenated_strips<R: Read + Seek>(
    reader: &mut R,
    offsets: &[u64],
    counts: &[u64],
) -> Result<Vec<u8>, DecodeError> {
    let total: usize = counts.iter().try_fold(0usize, |acc, &c| {
        acc.checked_add(c as usize)
            .ok_or(DecodeError::CorruptData("size overflow"))
    })?;
    let mut buf = vec![0u8; total];
    let mut pos = 0usize;
    for (off, cnt) in offsets.iter().zip(counts.iter()) {
        reader.seek(SeekFrom::Start(*off))?;
        reader.read_exact(&mut buf[pos..pos + *cnt as usize])?;
        pos += *cnt as usize;
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn build_sony_tone_curve_matches_dcraw() {
        // SonyToneCurve from an A7 IV ARW2: [8000, 10400, 12900, 14100].
        // dcraw/LibRaw: sony_curve[i+1] = (point >> 2) & 0xfff, bracketed by 0 and 4095,
        // giving {0, 2000, 2600, 3225, 3525, 4095}; segment i increments by (1 << i).
        let curve = build_sony_tone_curve([8000, 10400, 12900, 14100]);
        assert_eq!(curve[0], 0);
        assert_eq!(curve[1], 1); // segment 0, step 1
        assert_eq!(curve[2000], 2000); // end of segment 0
        assert_eq!(curve[2001], 2002); // segment 1, step 2
        assert_eq!(curve[2600], 3200); // end of segment 1
        assert_eq!(curve[4094], 17204); // max real index: pix 0x7ff -> (0x7ff << 1) = 0xffe
    }

    #[test]
    fn build_sony_tone_curve_identity_and_nonmonotonic() {
        // Missing tag (all zero) -> identity passthrough.
        let flat = build_sony_tone_curve([0, 0, 0, 0]);
        assert_eq!(flat[0], 0);
        assert_eq!(flat[1000], 1000);
        assert_eq!(flat[4094], 4094);
        // Non-monotonic control points must not panic; unset entries stay 0.
        let curve = build_sony_tone_curve([10400, 8000, 12900, 14100]);
        assert_eq!(curve[0], 0);
    }

    // Build one 16-byte ARW2 block whose 16 decoded pixels are all `value`.
    // header: max=min=value, imax=0, imin=1 -> pix[0]=max, pix[1]=min, and every other
    // pixel decodes code=0 (the 12 payload bytes are zero) -> (0 << sh) + min = value.
    fn make_uniform_block(value: u16) -> [u8; 16] {
        let v = (value & 0x7ff) as u32;
        let header = v | (v << 11) | (0u32 << 22) | (1u32 << 26);
        let mut block = [0u8; 16];
        block[0..4].copy_from_slice(&header.to_le_bytes());
        block
    }

    #[test]
    fn arw2_deinterleaves_columns_and_applies_tone_curve() {
        // One 32x1 row = block0 (value A) + block1 (value B). Correct ARW2 decoding writes
        // block0 to even columns and block1 to odd columns, after expanding through the curve
        // (indexed by pix << 1). Use an identity curve so curve[p << 1] == p << 1, which makes
        // both the interleave AND the `<< 1` indexing observable in the assertions.
        let dims = Dimensions {
            raw_width: 32,
            raw_height: 1,
            output_width: 32,
            output_height: 1,
        };
        let identity: Vec<u16> = (0..0x4000u32).map(|i| i as u16).collect();

        let a: u16 = 100;
        let b: u16 = 50;
        let mut row = Vec::with_capacity(32);
        row.extend_from_slice(&make_uniform_block(a));
        row.extend_from_slice(&make_uniform_block(b));

        let mut cur = Cursor::new(row);
        let res = sony_arw2_load_raw(&mut cur, dims, &identity).unwrap();

        for col in 0..32usize {
            let expected = if col % 2 == 0 {
                (a as u16) << 1 // even columns come from block0
            } else {
                (b as u16) << 1 // odd columns come from block1
            };
            assert_eq!(res.pixels[col], expected, "column {col}");
        }
        assert_eq!(res.white_level, 0x3ff0);
    }
}
