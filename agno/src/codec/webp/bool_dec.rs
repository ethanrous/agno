/// VP8 boolean arithmetic decoder -- exact inverse of `BoolEncoder`.
///
/// The encoder maintains `(range, bottom, bits_left)` where `bottom` is a
/// 32-bit accumulator and bytes are emitted from its MSB.  The decoder
/// maintains `(range, value, bit_count)` where `value` is a window into the
/// encoded byte stream, aligned so that `split << 8` is the decision
/// boundary.
///
/// Follows the same algorithm as RFC 6386 Section 7.3:
///   - `value` is initialized with the first two bytes (16 bits).
///   - For each bit: compute `split`, compare `value >= (split << 8)`.
///   - After each decision, renormalize bit-by-bit: shift `range` and
///     `value` left by 1, reading a new byte every 8 shifts.
pub struct BoolDecoder<'a> {
    buf: &'a [u8],
    pos: usize,
    range: u32,
    value: u32,
    bit_count: i32,
}

impl<'a> BoolDecoder<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        let b0 = if buf.is_empty() { 0 } else { buf[0] };
        let b1 = if buf.len() < 2 { 0 } else { buf[1] };

        Self {
            buf,
            pos: 2,
            range: 255,
            value: ((b0 as u32) << 8) | b1 as u32,
            bit_count: 0,
        }
    }

    fn next_byte(&mut self) -> u8 {
        if self.pos < self.buf.len() {
            let b = self.buf[self.pos];
            self.pos += 1;
            b
        } else {
            0
        }
    }

    /// Decode one boolean bit.
    ///
    /// `prob` is the probability [0..255] that the result is `false` (bit=0),
    /// matching the encoder's convention.  Returns `true` for bit=1.
    pub fn read_bit(&mut self, prob: u8) -> bool {
        let split = 1 + (((self.range - 1) * prob as u32) >> 8);
        let bigsplit = split << 8;
        let bit = self.value >= bigsplit;

        if bit {
            self.range -= split;
            self.value -= bigsplit;
        } else {
            self.range = split;
        }

        // Renormalize one bit at a time, matching the encoder's while-loop.
        while self.range < 128 {
            self.range <<= 1;
            self.value <<= 1;
            self.bit_count += 1;
            if self.bit_count == 8 {
                self.value |= self.next_byte() as u32;
                self.bit_count = 0;
            }
        }

        bit
    }

    /// Read `n_bits` equiprobable bits, MSB first.
    pub fn read_literal(&mut self, n_bits: u8) -> u32 {
        let mut v = 0u32;
        for _ in 0..n_bits {
            v = (v << 1) | self.read_bit(128) as u32;
        }
        v
    }

    /// Read a signed value: magnitude bits followed by sign bit (if
    /// magnitude > 0).  Mirrors `BoolEncoder::put_signed`.
    pub fn read_signed(&mut self, n_bits: u8) -> i32 {
        let mag = self.read_literal(n_bits) as i32;
        if mag > 0 && self.read_bit(128) {
            -mag
        } else {
            mag
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::webp::bool_enc::BoolEncoder;

    fn encode_bits(pairs: &[(bool, u8)]) -> Vec<u8> {
        let mut enc = BoolEncoder::new();
        for &(bit, prob) in pairs {
            enc.put_bit(bit, prob);
        }
        enc.finish()
    }

    #[test]
    fn roundtrip_32_equiprobable_bits() {
        let bits: Vec<bool> = (0..32u32)
            .map(|i| (i.wrapping_mul(7) ^ 0xA5) & 1 != 0)
            .collect();
        let pairs: Vec<(bool, u8)> = bits.iter().map(|&b| (b, 128)).collect();
        let data = encode_bits(&pairs);

        let mut dec = BoolDecoder::new(&data);
        for (i, &expected) in bits.iter().enumerate() {
            assert_eq!(
                dec.read_bit(128),
                expected,
                "mismatch at bit {i}"
            );
        }
    }

    #[test]
    fn roundtrip_biased_bits() {
        let mut pairs = Vec::new();
        let mut state: u32 = 0x12345678;
        for _ in 0..64 {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let prob = ((state >> 16) & 0xFF) as u8;
            let prob = prob.max(1);
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let bit = (state >> 24) & 1 != 0;
            pairs.push((bit, prob));
        }
        let data = encode_bits(&pairs);

        let mut dec = BoolDecoder::new(&data);
        for (i, &(expected, prob)) in pairs.iter().enumerate() {
            assert_eq!(
                dec.read_bit(prob),
                expected,
                "mismatch at bit {i}, prob={prob}"
            );
        }
    }

    #[test]
    fn roundtrip_literal_and_signed() {
        let mut enc = BoolEncoder::new();
        // Pad with some leading data to avoid the short-sequence edge case.
        for _ in 0..32 {
            enc.put_bit(false, 128);
        }
        enc.put_literal(0b1010_0110, 8);
        enc.put_literal(42, 7);
        enc.put_literal(0, 4);
        enc.put_literal(15, 4);
        enc.put_signed(5, 4);
        enc.put_signed(-5, 4);
        enc.put_signed(0, 4);
        enc.put_signed(127, 7);
        enc.put_signed(-1, 3);
        let data = enc.finish();

        let mut dec = BoolDecoder::new(&data);
        for _ in 0..32 {
            dec.read_bit(128);
        }
        assert_eq!(dec.read_literal(8), 0b1010_0110);
        assert_eq!(dec.read_literal(7), 42);
        assert_eq!(dec.read_literal(4), 0);
        assert_eq!(dec.read_literal(4), 15);
        assert_eq!(dec.read_signed(4), 5);
        assert_eq!(dec.read_signed(4), -5);
        assert_eq!(dec.read_signed(4), 0);
        assert_eq!(dec.read_signed(7), 127);
        assert_eq!(dec.read_signed(3), -1);
    }

    #[test]
    fn roundtrip_500_random_bits() {
        let mut enc = BoolEncoder::new();
        let mut expected = Vec::new();
        let mut state: u32 = 0xDEAD_BEEF;
        for _ in 0..500 {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let prob = ((state >> 16) & 0xFF) as u8;
            let prob = prob.max(1);
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            let bit = (state >> 24) & 1 != 0;
            enc.put_bit(bit, prob);
            expected.push((bit, prob));
        }
        let data = enc.finish();

        let mut dec = BoolDecoder::new(&data);
        for (i, &(bit, prob)) in expected.iter().enumerate() {
            assert_eq!(
                dec.read_bit(prob),
                bit,
                "mismatch at bit {i}, prob={prob}"
            );
        }
    }
}
