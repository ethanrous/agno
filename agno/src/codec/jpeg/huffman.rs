use std::io::Write;

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
    if v == 0 {
        0
    } else {
        16 - v.leading_zeros() as u8
    }
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
        self.emit_bits(
            table.codes[category as usize],
            table.sizes[category as usize],
        )?;

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
    pub fn write_ac_block(
        &mut self,
        coeffs: &[i16; 63],
        table: &HuffmanLut,
    ) -> std::io::Result<()> {
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
