/// VP8 boolean arithmetic encoder (RFC 6386 Section 7).
///
/// Matches libvpx's `vp8_encode_bool` implementation for correct carry
/// handling.  The encoder tracks a `carry_count` for runs of 0xFF bytes
/// that might need to absorb a future carry.
pub struct BoolEncoder {
    output: Vec<u8>,
    range: u32,
    bottom: u32,
    /// Number of bits shifted into `bottom` since the last byte emission.
    /// Starts at 24 (the encoder can buffer up to 24 bits before emitting
    /// the first byte).  After a byte is emitted, it resets to track the
    /// new batch.
    count: i32,
    /// Number of stacked 0xFF bytes whose final value depends on whether
    /// a carry propagates into them.
    carry_count: u32,
    /// The last non-0xFF byte emitted.  When a carry arrives, this byte
    /// is incremented and all pending 0xFF bytes become 0x00.
    pre_carry_byte: u8,
    /// Whether any byte has been committed to `output` yet.
    started: bool,
}

impl BoolEncoder {
    pub fn new() -> Self {
        Self {
            output: Vec::new(),
            range: 255,
            bottom: 0,
            count: 24,
            carry_count: 0,
            pre_carry_byte: 0,
            started: false,
        }
    }

    /// Encode one bit with given probability (0-255, prob of bit being 0).
    pub fn put_bit(&mut self, bit: bool, prob: u8) {
        let split = 1 + (((self.range - 1) * prob as u32) >> 8);
        if bit {
            self.bottom += split;
            self.range -= split;
        } else {
            self.range = split;
        }
        self.renormalize();
    }

    fn renormalize(&mut self) {
        // Compute the total number of shifts needed to bring range back
        // into [128..255].
        while self.range < 128 {
            self.range <<= 1;
            self.count -= 1;

            if self.count < 0 {
                // Time to emit a byte.
                let byte = (self.bottom >> 24) as u8;
                self.emit_byte(byte);
                self.bottom &= 0x00FF_FFFF;
                self.count += 8;
            }

            self.bottom <<= 1;
        }
    }

    /// Emit a byte using the carry-safe "stacking" strategy.
    ///
    /// A 0xFF byte cannot absorb a carry without propagating it further, so
    /// consecutive 0xFF bytes are stacked rather than written immediately.
    /// When the next non-0xFF byte arrives, the stack is flushed (all 0xFF
    /// bytes are now safe because the non-0xFF byte breaks the carry chain).
    fn emit_byte(&mut self, byte: u8) {
        if !self.started {
            self.pre_carry_byte = byte;
            self.started = true;
        } else if byte == 0xFF {
            self.carry_count += 1;
        } else {
            self.output.push(self.pre_carry_byte);
            for _ in 0..self.carry_count {
                self.output.push(0xFF);
            }
            self.carry_count = 0;
            self.pre_carry_byte = byte;
        }
    }

    /// Encode n_bits of value MSB-first, each with prob=128 (equiprobable).
    pub fn put_literal(&mut self, value: u32, n_bits: u8) {
        for i in (0..n_bits).rev() {
            self.put_bit((value >> i) & 1 != 0, 128);
        }
    }

    /// Encode signed value: magnitude bits followed by sign bit.
    #[cfg(test)]
    pub fn put_signed(&mut self, value: i32, n_bits: u8) {
        if n_bits == 0 {
            return;
        }
        let mag = value.unsigned_abs();
        self.put_literal(mag, n_bits);
        if mag > 0 {
            self.put_bit(value < 0, 128);
        }
    }

    /// Flush remaining bits and return the encoded data.
    pub fn finish(mut self) -> Vec<u8> {
        // Flush by encoding 32 zero bits to push all buffered data out.
        for _ in 0..32 {
            self.put_bit(false, 128);
        }
        // Write out remaining bytes.
        if self.started {
            self.output.push(self.pre_carry_byte);
            for _ in 0..self.carry_count {
                self.output.push(0xFF);
            }
        }
        self.output
    }
}
