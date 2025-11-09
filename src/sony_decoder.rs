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

// pub trait SonyDecrypt {
//     fn decrypt_u32_words(&mut self, data_32bit_words: &mut [u32], start_sequence: bool, key: u32);
// }

// Direct port of LibRaw::sony_decrypt working on u32 words (LE-packed).
// pub struct SonyDecryptor {
//     pad: [u32; 128],
//     p: u32,
// }

// impl SonyDecryptor {
//     pub fn new() -> Self {
//         Self {
//             pad: [0; 128],
//             p: 0,
//         }
//     }
//
//     fn init_pad(&mut self, key: u32) {
//         let mut k = key as u64;
//         for i in 0..4 {
//             k = k.wrapping_mul(48_828_125u64).wrapping_add(1);
//             self.pad[i] = k as u32;
//         }
//         // pad[3] = pad[3] << 1 | (pad[0] ^ pad[2]) >> 31;
//         self.pad[3] = (self.pad[3] << 1) | (((self.pad[0] ^ self.pad[2]) >> 31) & 1);
//
//         for i in 4..127 {
//             // (pad[p-4] ^ pad[p-2]) << 1 | (pad[p-3] ^ pad[p-1]) >> 31
//             let left = (self.pad[i - 4] ^ self.pad[i - 2]) << 1;
//             let right = ((self.pad[i - 3] ^ self.pad[i - 1]) >> 31) & 1;
//             self.pad[i] = left | right;
//         }
//         // htonl equivalent
//         for i in 0..127 {
//             self.pad[i] = self.pad[i].to_be();
//         }
//         self.p = 0;
//     }
// }

// impl SonyDecrypt for SonyDecryptor {
//     fn decrypt_u32_words(&mut self, data_32bit_words: &mut [u32], start_sequence: bool, key: u32) {
//         if start_sequence {
//             self.init_pad(key);
//         }
//         for w in data_32bit_words.iter_mut() {
//             // Advance pad stream: pad[p&127] = pad[(p+1)&127] ^ pad[(p+65)&127]
//             let idx = (self.p & 127) as usize;
//             let i1 = ((self.p + 1) & 127) as usize;
//             let i2 = ((self.p + 65) & 127) as usize;
//             self.pad[idx] = self.pad[i1] ^ self.pad[i2];
//             *w ^= self.pad[idx];
//             self.p = self.p.wrapping_add(1);
//         }
//     }
// }

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

// Port of LibRaw::sony_arw2_load_raw (block-based: 16 bytes -> 16 pixels)
// For each row, the stream contains 16-byte blocks:
//   - First 4 bytes (LE) carry fields: max(11b), min(11b), imax(4b), imin(4b)
//   - Remaining 12 bytes carry 14 packed 7-bit codes, MSB-first within the 16-byte span:
//       starting at bit offset 30, each 7-bit code -> value = (code << sh) + min
//       positions imax/imin are set to max/min respectively.
// We decode blocks until we fill active_width pixels. Any trailing row bytes are ignored.
pub fn sony_arw2_load_raw<R: Read>(
    reader: &mut R,
    dims: Dimensions,
) -> Result<SonyLoadResult, DecodeError> {
    let row_len = dims.output_width; // bytes per compressed row in ARW2 equal to pixel width
    let mut pixels = vec![0u16; dims.raw_width * dims.raw_height];

    let mut row_buf = vec![0u8; row_len + 1];

    for row in 0..dims.output_height {
        // Read one row of compressed bytes
        reader.read_exact(&mut row_buf[..row_len])?;

        let mut out_col = 0usize;
        let mut dp = 0usize;

        while out_col < dims.output_width && dp + 16 <= row_len {
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
            let mut bit = 30;

            for i in 0..16usize {
                if i == imax {
                    pix16[i] = max_v as u16;
                } else if i == imin {
                    pix16[i] = min_v as u16;
                } else {
                    let byte_index = dp + ((bit >> 3) as usize);
                    if byte_index + 1 >= row_buf.len() {
                        return Err(DecodeError::CorruptData("Sony ARW2: row buffer overread"));
                    }
                    let two =
                        u16::from_le_bytes([row_buf[byte_index], row_buf[byte_index + 1]]) as i32;
                    let code7 = (two >> (bit & 7)) & 0x7f;
                    let value = ((code7 << sh) + min_v) as i32;
                    pix16[i] = value as u16;
                    bit += 7;
                }
            }

            let run = std::cmp::min(16, dims.output_width - out_col);
            for i in 0..run {
                let dst = row * dims.raw_width + (out_col + i);
                pixels[dst] = pix16[i];
            }

            out_col += 16;
            dp += 16;
        }
    }

    Ok(SonyLoadResult {
        pixels,
        white_level: 0x3fff,
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
