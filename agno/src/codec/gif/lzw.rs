//! GIF LZW decompressor. See GIF89a spec Appendix F.
//!
//! GIF LZW differs from generic LZW in two ways:
//! 1. Codes are packed LSB-first across bytes (unlike most LZW which is MSB-first).
//! 2. Compressed data is wrapped in sub-blocks (1-byte length prefix, terminated by a zero-length block).

use std::error::Error;

const MAX_CODE_SIZE: u8 = 12;
const MAX_DICT: usize = 1 << MAX_CODE_SIZE; // 4096

struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_buf: u32,
    bit_count: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_buf: 0,
            bit_count: 0,
        }
    }

    fn read(&mut self, n: u8) -> Option<u32> {
        while self.bit_count < n {
            if self.byte_pos >= self.data.len() {
                if self.bit_count < n {
                    return None;
                }
                break;
            }
            self.bit_buf |= (self.data[self.byte_pos] as u32) << self.bit_count;
            self.byte_pos += 1;
            self.bit_count += 8;
        }
        let mask = (1u32 << n) - 1;
        let value = self.bit_buf & mask;
        self.bit_buf >>= n;
        self.bit_count -= n;
        Some(value)
    }
}

/// Decompress a GIF LZW data stream.
///
/// `min_code_size` is the value read from the GIF Image Data block header
/// (always 2..=8 in valid GIFs). `data` is the *concatenated* sub-block payload
/// (sub-block length prefixes already stripped by the caller). `max_output`
/// caps the decompressed byte count so a corrupt or malicious stream cannot
/// force unbounded allocation.
///
/// Returns the raw palette indices, one byte per pixel.
pub fn decompress_lzw(
    min_code_size: u8,
    data: &[u8],
    max_output: usize,
) -> Result<Vec<u8>, Box<dyn Error>> {
    if !(2..=8).contains(&min_code_size) {
        return Err(format!("GIF LZW min_code_size {min_code_size} out of range 2..=8").into());
    }

    let clear_code = 1u32 << min_code_size;
    let eoi_code = clear_code + 1;

    // Dict stored as three parallel tables. `length` lets us size the output slice in O(1)
    // and write the expansion directly without a scratch buffer. Chain is walked via prefix.
    let mut prefix: Vec<u32> = vec![0; MAX_DICT];
    let mut suffix: Vec<u8> = vec![0; MAX_DICT];
    let mut length: Vec<u16> = vec![0; MAX_DICT];
    for i in 0..clear_code as usize {
        prefix[i] = u32::MAX;
        suffix[i] = i as u8;
        length[i] = 1;
    }

    let mut code_size = min_code_size + 1;
    let mut next_code = eoi_code + 1;

    let mut reader = BitReader::new(data);
    let mut output: Vec<u8> = Vec::with_capacity(max_output.min(data.len() * 3));
    let mut prev_code: Option<u32> = None;

    while let Some(code) = reader.read(code_size) {
        if code == eoi_code {
            break;
        }

        if code == clear_code {
            for i in 0..clear_code as usize {
                prefix[i] = u32::MAX;
                suffix[i] = i as u8;
                length[i] = 1;
            }
            code_size = min_code_size + 1;
            next_code = eoi_code + 1;
            prev_code = None;
            continue;
        }

        if code > next_code {
            return Err(format!("Invalid LZW code {code}: exceeds next_code {next_code}").into());
        }
        if prev_code.is_none() && code >= clear_code {
            return Err(format!("First LZW code {code} is not a root code").into());
        }

        // KwKwK: the entry being read is the one about to be defined. Its expansion is
        // prev's expansion followed by first_byte_of_prev — one byte longer than prev.
        let kwkwk = code == next_code;
        let (walk, walk_len) = if kwkwk {
            let pc = prev_code.unwrap();
            (pc, length[pc as usize] as usize)
        } else {
            (code, length[code as usize] as usize)
        };
        let entry_len = walk_len + if kwkwk { 1 } else { 0 };

        let start = output.len();
        output.resize(start + entry_len, 0);
        if output.len() > max_output {
            return Err(format!("GIF LZW output exceeds expected size {max_output}").into());
        }

        // Walk the prefix chain backwards, filling the output slice from the tail.
        let mut cur = walk;
        for i in (0..walk_len).rev() {
            output[start + i] = suffix[cur as usize];
            let p = prefix[cur as usize];
            if p != u32::MAX {
                cur = p;
            }
        }
        let emit_first = output[start];
        if kwkwk {
            output[start + walk_len] = emit_first;
        }

        if let Some(pc) = prev_code
            && next_code < MAX_DICT as u32
        {
            prefix[next_code as usize] = pc;
            suffix[next_code as usize] = emit_first;
            length[next_code as usize] = length[pc as usize] + 1;
            next_code += 1;

            if next_code == (1u32 << code_size) && code_size < MAX_CODE_SIZE {
                code_size += 1;
            }
        }

        prev_code = Some(code);
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lzw_single_index_zero() {
        // Fixture A from the plan: clear(4), 0, eoi(5), 3-bit codes, LSB-first → bytes 0x44, 0x01
        let out = decompress_lzw(2, &[0x44, 0x01], 1024).unwrap();
        assert_eq!(out, vec![0]);
    }

    #[test]
    fn lzw_kwkwk_case() {
        // Fixture B from the plan: hand-rolled stream that exercises the KwKwK code path.
        // Codes [clear(4), 0, 6, eoi(5)] → output [0, 0, 0].
        // The decoder reads code 6 at the moment next_code == 6, so it must reconstruct
        // the entry from prev (=0) plus first_byte_of_prev (=0).
        let out = decompress_lzw(2, &[0x84, 0x0B], 1024).unwrap();
        assert_eq!(out, vec![0, 0, 0]);
    }

    #[test]
    fn lzw_min_code_size_out_of_range_errors() {
        // GIF LZW min_code_size must be 2..=8
        assert!(decompress_lzw(1, &[0x00], 1024).is_err());
        assert!(decompress_lzw(9, &[0x00], 1024).is_err());
    }

    #[test]
    fn lzw_corrupt_code_errors() {
        // A code that's larger than next_code (and not equal to next_code) is invalid.
        // We construct one by emitting clear, then a code that doesn't exist in the initial dictionary.
        // min_code_size=2 → clear=4, eoi=5, initial dict 0..=3, so code 6 is invalid right after clear.
        // Codes: clear(4)=100, 6=110, ... → bit stream: 0,0,1, 0,1,1
        // byte0 low→high: 0,0,1,0,1,1,0,0 = 0b00110100 = 0x34
        let result = decompress_lzw(2, &[0x34], 1024);
        assert!(result.is_err());
    }

    #[test]
    fn lzw_output_cap_enforced() {
        // Same valid stream as lzw_single_index_zero (decodes to one byte) but with
        // max_output=0 — decoder must reject before emitting the byte.
        let err = decompress_lzw(2, &[0x44, 0x01], 0);
        assert!(err.is_err(), "expected error when max_output=0");
    }

    #[test]
    fn lzw_round_trip_via_image_crate() {
        // Use the `image` crate's GIF encoder to produce a known-good LZW stream and verify
        // our decoder agrees. A linear scan for 0x2C would be unsafe because that byte may
        // appear inside a color table or extension payload, so we walk blocks properly.
        use image::{Frame, RgbaImage, codecs::gif::GifEncoder};
        use std::io::Cursor;

        let mut rgba = RgbaImage::new(16, 1);
        for x in 0..16u32 {
            let v = (x * 16) as u8;
            rgba.put_pixel(x, 0, image::Rgba([v, v, v, 255]));
        }

        let mut encoded = Vec::new();
        {
            let mut enc = GifEncoder::new(Cursor::new(&mut encoded));
            enc.encode_frame(Frame::new(rgba.clone())).unwrap();
        }

        assert!(&encoded[..6] == b"GIF87a" || &encoded[..6] == b"GIF89a");
        let lsd_packed = encoded[10];
        let mut p = 13usize;
        if lsd_packed & 0x80 != 0 {
            let n = (lsd_packed & 0x07) as u32;
            p += 3 * (1usize << (n + 1));
        }
        let img_desc_pos = loop {
            let intro = encoded[p];
            if intro == 0x2C {
                break p;
            } else if intro == 0x21 {
                p += 2;
                loop {
                    let len = encoded[p] as usize;
                    p += 1;
                    if len == 0 {
                        break;
                    }
                    p += len;
                }
            } else if intro == 0x3B {
                panic!("no image descriptor in encoded GIF");
            } else {
                panic!("unknown block introducer 0x{intro:02X} at {p}");
            }
        };
        let packed = encoded[img_desc_pos + 9];
        let local_table_present = packed & 0x80 != 0;
        let local_table_size = if local_table_present {
            3 * (1 << ((packed & 0x07) + 1))
        } else {
            0
        };
        let mut p = img_desc_pos + 10 + local_table_size;
        let min_code_size = encoded[p];
        p += 1;
        let mut payload = Vec::new();
        loop {
            let len = encoded[p] as usize;
            p += 1;
            if len == 0 {
                break;
            }
            payload.extend_from_slice(&encoded[p..p + len]);
            p += len;
        }

        let indices = decompress_lzw(min_code_size, &payload, 1024).unwrap();
        assert_eq!(indices.len(), 16);
        let palette_entries = if local_table_present {
            1usize << ((packed & 0x07) + 1)
        } else {
            assert!(lsd_packed & 0x80 != 0, "no global or local color table");
            1usize << ((lsd_packed & 0x07) + 1)
        };
        for (i, &idx) in indices.iter().enumerate() {
            assert!(
                (idx as usize) < palette_entries,
                "pixel {i} index {idx} out of palette"
            );
        }
    }
}
