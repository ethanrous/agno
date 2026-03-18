use std::error::Error;
use std::io::Write;

// ---- Decoding ----

/// Huffman decode table for JPEG decoding.
/// Uses min/max code ranges per bit length for O(code_length) symbol lookup.
pub struct HuffmanDecodeTable {
    /// Minimum code value at each bit length (1-indexed; index 0 is unused).
    min_code: [i32; 17],
    /// Maximum code value at each bit length (1-indexed; index 0 is unused).
    max_code: [i32; 17],
    /// Index into `values` for the first symbol of each bit length.
    val_ptr: [usize; 17],
    /// Symbol values in canonical Huffman order.
    values: Vec<u8>,
}

/// Build a Huffman decode table from BITS[16] and VALUES arrays (JPEG Annex C).
pub fn build_huffman_decode_table(bits: &[u8; 16], values: &[u8]) -> HuffmanDecodeTable {
    let mut min_code = [-1i32; 17];
    let mut max_code = [-1i32; 17];
    let mut val_ptr = [0usize; 17];

    let mut code = 0i32;
    let mut vi = 0usize;

    for len in 1..=16usize {
        let count = bits[len - 1] as usize;
        if count > 0 {
            val_ptr[len] = vi;
            min_code[len] = code;
            code += count as i32;
            max_code[len] = code - 1;
            vi += count;
        }
        code <<= 1;
    }

    HuffmanDecodeTable {
        min_code,
        max_code,
        val_ptr,
        values: values.to_vec(),
    }
}

impl HuffmanDecodeTable {
    /// Decode one Huffman symbol from the bit reader.
    pub fn decode(&self, reader: &mut JpegBitReader) -> Result<u8, Box<dyn Error>> {
        let mut code = 0i32;
        for len in 1..=16usize {
            code = (code << 1) | reader.read_bit()? as i32;
            if self.max_code[len] >= 0 && code <= self.max_code[len] {
                let idx = self.val_ptr[len] + (code - self.min_code[len]) as usize;
                return Ok(self.values[idx]);
            }
        }
        Err("Invalid Huffman code (no match in 16 bits)".into())
    }
}

/// Bit-level reader for JPEG entropy-coded data.
///
/// Handles byte stuffing (`0xFF 0x00` -> literal `0xFF`) and
/// restart markers (`0xFF 0xD0`..`0xFF 0xD7`).
pub struct JpegBitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit_buffer: u32,
    bits_left: u8,
}

impl<'a> JpegBitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bit_buffer: 0,
            bits_left: 0,
        }
    }

    /// Read the next byte from the entropy-coded stream, handling byte stuffing.
    fn next_byte(&mut self) -> Result<u8, Box<dyn Error>> {
        if self.pos >= self.data.len() {
            return Err("Unexpected end of JPEG data".into());
        }
        let byte = self.data[self.pos];
        self.pos += 1;

        if byte == 0xFF {
            if self.pos >= self.data.len() {
                return Err("Unexpected end of JPEG data after 0xFF".into());
            }
            let next = self.data[self.pos];
            if next == 0x00 {
                self.pos += 1;
                Ok(0xFF)
            } else if (0xD0..=0xD7).contains(&next) {
                // Restart marker encountered during byte read -- skip it
                self.pos += 1;
                self.next_byte()
            } else {
                // End of entropy data (likely EOI or next marker)
                self.pos -= 1; // back up so caller can see the marker
                Err(format!("Marker 0xFF{:02X} in entropy data", next).into())
            }
        } else {
            Ok(byte)
        }
    }

    /// Read a single bit (MSB first).
    pub fn read_bit(&mut self) -> Result<u8, Box<dyn Error>> {
        if self.bits_left == 0 {
            self.bit_buffer = self.next_byte()? as u32;
            self.bits_left = 8;
        }
        self.bits_left -= 1;
        Ok(((self.bit_buffer >> self.bits_left) & 1) as u8)
    }

    /// Read `n` bits as a u32 (MSB first).
    pub fn read_bits(&mut self, n: u8) -> Result<u32, Box<dyn Error>> {
        let mut val = 0u32;
        for _ in 0..n {
            val = (val << 1) | self.read_bit()? as u32;
        }
        Ok(val)
    }

    /// Discard any remaining bits in the current byte (align to byte boundary).
    pub fn align(&mut self) {
        self.bits_left = 0;
    }

    /// Advance past the next restart marker in the stream.
    /// Returns the restart marker index (0-7), or an error if none found.
    pub fn skip_to_restart(&mut self) -> Result<u8, Box<dyn Error>> {
        self.align();
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xFF {
                let marker = self.data[self.pos + 1];
                if (0xD0..=0xD7).contains(&marker) {
                    self.pos += 2;
                    return Ok(marker - 0xD0);
                }
            }
            self.pos += 1;
        }
        Err("No restart marker found".into())
    }
}

// ---- Encoding ----

/// Precomputed Huffman code/size lookup table for JPEG encoding.
pub struct HuffmanLut {
    pub codes: [u16; 256],
    pub sizes: [u8; 256],
}

/// Build a Huffman lookup table from BITS and VALUES arrays per JPEG Annex C.
pub fn build_huffman_lut(bits: &[u8; 16], values: &[u8]) -> HuffmanLut {
    let mut codes = [0u16; 256];
    let mut sizes = [0u8; 256];

    let mut code: u16 = 0;
    let mut vi = 0; // index into values

    for (bit_len_minus_1, &count) in bits.iter().enumerate() {
        let bit_len = (bit_len_minus_1 + 1) as u8;
        for _ in 0..count {
            if vi < values.len() {
                let symbol = values[vi] as usize;
                codes[symbol] = code;
                sizes[symbol] = bit_len;
                vi += 1;
            }
            code += 1;
        }
        code <<= 1;
    }

    HuffmanLut { codes, sizes }
}

/// Number of bits needed to represent a value (0 -> 0, 1 -> 1, 2..3 -> 2, etc.)
fn bit_length(v: u16) -> u8 {
    if v == 0 { 0 } else { 16 - v.leading_zeros() as u8 }
}

/// Bit-level writer with JPEG byte stuffing (0xFF -> 0xFF 0x00).
pub struct JpegBitWriter<W: Write> {
    writer: W,
    bit_buffer: u32,
    bits_in_buffer: u8,
}

impl<W: Write> JpegBitWriter<W> {
    pub fn new(writer: W) -> Self {
        JpegBitWriter {
            writer,
            bit_buffer: 0,
            bits_in_buffer: 0,
        }
    }

    /// Write `size` MSBs of `value` into the bit stream.
    fn emit_bits(&mut self, value: u16, size: u8) -> std::io::Result<()> {
        // Mask to only the relevant bits
        let v = value & ((1u32 << size as u32) - 1) as u16;

        // Shift new bits into the buffer (MSB-first packing)
        self.bit_buffer = (self.bit_buffer << size) | v as u32;
        self.bits_in_buffer += size;

        // Flush complete bytes from the top
        while self.bits_in_buffer >= 8 {
            self.bits_in_buffer -= 8;
            let byte = ((self.bit_buffer >> self.bits_in_buffer) & 0xFF) as u8;
            self.writer.write_all(&[byte])?;
            if byte == 0xFF {
                self.writer.write_all(&[0x00])?;
            }
        }

        // Keep only the remaining bits
        if self.bits_in_buffer > 0 {
            self.bit_buffer &= (1u32 << self.bits_in_buffer) - 1;
        } else {
            self.bit_buffer = 0;
        }

        Ok(())
    }

    /// Encode a DC coefficient difference.
    pub fn write_dc(&mut self, diff: i16, table: &HuffmanLut) -> std::io::Result<()> {
        let abs_val = diff.unsigned_abs();
        let category = bit_length(abs_val);

        // Huffman code for the category
        self.emit_bits(table.codes[category as usize], table.sizes[category as usize])?;

        // Actual value bits (ones-complement for negatives)
        if category > 0 {
            let bits = if diff < 0 {
                (diff - 1) as u16 & ((1u16 << category) - 1)
            } else {
                diff as u16
            };
            self.emit_bits(bits, category)?;
        }

        Ok(())
    }

    /// Encode 63 AC coefficients (zigzag-ordered, indices 1..63).
    pub fn write_ac_block(&mut self, coeffs: &[i16; 63], table: &HuffmanLut) -> std::io::Result<()> {
        let mut zero_run: u8 = 0;

        for i in 0..63 {
            let val = coeffs[i];
            if val == 0 {
                zero_run += 1;
                continue;
            }

            // Emit ZRL (0xF0) for runs of 16+ zeros
            while zero_run >= 16 {
                self.emit_bits(table.codes[0xF0], table.sizes[0xF0])?;
                zero_run -= 16;
            }

            let abs_val = val.unsigned_abs();
            let category = bit_length(abs_val);
            let symbol = (zero_run << 4) | category;

            self.emit_bits(table.codes[symbol as usize], table.sizes[symbol as usize])?;

            let bits = if val < 0 {
                (val - 1) as u16 & ((1u16 << category) - 1)
            } else {
                val as u16
            };
            self.emit_bits(bits, category)?;

            zero_run = 0;
        }

        // EOB if last coefficient(s) were zero
        if zero_run > 0 {
            self.emit_bits(table.codes[0x00], table.sizes[0x00])?;
        }

        Ok(())
    }

    /// Pad remaining bits with 1s and flush to byte boundary.
    pub fn flush(&mut self) -> std::io::Result<()> {
        if self.bits_in_buffer > 0 {
            let pad = 8 - self.bits_in_buffer;
            // Pad with 1-bits (all ones)
            self.emit_bits((1u16 << pad) - 1, pad)?;
        }
        self.writer.flush()
    }

}
