use anyhow::{Result, bail, ensure};

// ---------------------------------------------------------------------------
// HEVC CABAC state transition tables (ITU-T H.265 Table 9-42)
// Used only by CabacReader tests; production CABAC lives in slice.rs.
// ---------------------------------------------------------------------------

/// Next probability state index after decoding the Least Probable Symbol.
/// Indexed by current pStateIdx (0..63).
#[cfg(test)]
#[rustfmt::skip]
const TRANS_IDX_LPS: [u8; 64] = [
     0,  0,  1,  2,  2,  4,  4,  5,  6,  7,  8,  9,  9, 11, 11, 12,
    13, 13, 15, 15, 16, 16, 18, 18, 19, 19, 21, 21, 22, 22, 23, 24,
    24, 25, 26, 26, 27, 27, 28, 29, 29, 30, 30, 30, 31, 32, 32, 33,
    33, 33, 34, 34, 35, 35, 35, 36, 36, 36, 37, 37, 37, 38, 38, 63,
];

/// Next probability state index after decoding the Most Probable Symbol.
/// Indexed by current pStateIdx (0..63).
#[cfg(test)]
#[rustfmt::skip]
const TRANS_IDX_MPS: [u8; 64] = [
     1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13, 14, 15, 16,
    17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32,
    33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48,
    49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 62, 63,
];

/// LPS range table (ITU-T H.265 Table 9-43).
/// Indexed as [pStateIdx][qRangeIdx] where qRangeIdx = (ivlCurrRange >> 6) & 3.
/// Each row has 4 entries for the 4 possible qRangeIdx values.
#[cfg(test)]
#[rustfmt::skip]
const RANGE_TAB_LPS: [[u16; 4]; 64] = [
    [128, 176, 208, 240], [128, 167, 197, 227], [128, 158, 187, 216],
    [123, 150, 178, 205], [116, 142, 169, 195], [111, 135, 160, 185],
    [105, 128, 152, 175], [100, 122, 144, 166], [ 95, 116, 137, 158],
    [ 90, 110, 130, 150], [ 85, 104, 123, 142], [ 81,  99, 117, 135],
    [ 77,  94, 111, 128], [ 73,  89, 105, 122], [ 69,  85, 100, 116],
    [ 66,  80,  95, 110], [ 62,  76,  90, 104], [ 59,  72,  86,  99],
    [ 56,  69,  81,  94], [ 53,  65,  77,  89], [ 51,  62,  73,  85],
    [ 48,  59,  69,  80], [ 46,  56,  66,  76], [ 43,  53,  63,  72],
    [ 41,  50,  59,  69], [ 39,  48,  56,  65], [ 37,  45,  54,  62],
    [ 35,  43,  51,  59], [ 33,  41,  48,  56], [ 32,  39,  46,  53],
    [ 30,  37,  43,  50], [ 29,  35,  41,  48], [ 27,  33,  39,  45],
    [ 26,  31,  37,  43], [ 24,  30,  35,  41], [ 23,  28,  33,  39],
    [ 22,  27,  32,  37], [ 21,  26,  30,  35], [ 20,  24,  29,  33],
    [ 19,  23,  27,  31], [ 18,  22,  26,  30], [ 17,  21,  25,  28],
    [ 16,  20,  23,  27], [ 15,  19,  22,  25], [ 14,  18,  21,  24],
    [ 14,  17,  20,  23], [ 13,  16,  19,  22], [ 12,  15,  18,  21],
    [ 12,  14,  17,  20], [ 11,  14,  16,  19], [ 11,  13,  15,  18],
    [ 10,  12,  15,  17], [ 10,  12,  14,  16], [  9,  11,  13,  15],
    [  9,  11,  12,  14], [  8,  10,  12,  14], [  8,   9,  11,  13],
    [  7,   9,  11,  12], [  7,   9,  10,  12], [  7,   8,  10,  11],
    [  6,   8,   9,  11], [  6,   7,   9,  10], [  6,   7,   8,   9],
    [  2,   2,   2,   2],
];

// ---------------------------------------------------------------------------
// BitReader -- raw NAL bitstream reader with emulation prevention
// ---------------------------------------------------------------------------

/// Reads bits from a byte slice representing an HEVC NAL unit payload.
///
/// Handles HEVC/H.264 emulation prevention: the byte sequence
/// `0x00 0x00 0x03` in the coded stream has the `0x03` byte removed
/// to recover the original RBSP (Raw Byte Sequence Payload).
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Byte position in the source data (includes emulation prevention bytes).
    byte_pos: usize,
    /// Bit offset within the current byte (0 = MSB, 7 = LSB).
    bit_pos: u8,
    /// Count of consecutive 0x00 bytes seen, for emulation prevention detection.
    num_zeros: u32,
    /// When true, skip emulation prevention byte removal (data is already RBSP).
    skip_ep: bool,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
            num_zeros: 0,
            skip_ep: false,
        }
    }

    /// Create a BitReader for already-cleaned RBSP data (no EP removal).
    pub fn new_rbsp(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
            num_zeros: 0,
            skip_ep: true,
        }
    }

    /// Returns the number of bits remaining in the stream (approximate -- does
    /// not account for emulation prevention bytes that have not yet been
    /// encountered; an exact count would require a full forward scan).
    pub fn bits_remaining(&self) -> usize {
        if self.byte_pos >= self.data.len() {
            return 0;
        }
        (self.data.len() - self.byte_pos) * 8 - self.bit_pos as usize
    }

    /// True when no more bits can be read.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.byte_pos >= self.data.len()
    }

    /// Read a single bit, handling emulation prevention.
    pub fn read_bit(&mut self) -> Result<u8> {
        ensure!(
            self.byte_pos < self.data.len(),
            "BitReader: read past end of data"
        );

        let mut current_byte = self.data[self.byte_pos];

        // At the start of a new byte, check for emulation prevention (0x00 0x00 0x03)
        if !self.skip_ep && self.bit_pos == 0 && self.num_zeros >= 2 && current_byte == 0x03 {
            self.byte_pos += 1;
            ensure!(
                self.byte_pos < self.data.len(),
                "BitReader: emulation prevention byte at end of data"
            );
            self.num_zeros = 0;
            current_byte = self.data[self.byte_pos];
        }

        let bit = (current_byte >> (7 - self.bit_pos)) & 1;

        self.bit_pos += 1;
        if self.bit_pos == 8 {
            if current_byte == 0x00 {
                self.num_zeros += 1;
            } else {
                self.num_zeros = 0;
            }
            self.bit_pos = 0;
            self.byte_pos += 1;
        }

        Ok(bit)
    }

    /// Read `n` bits (0..=32) and return them right-aligned in a u32.
    pub fn read_bits(&mut self, n: u8) -> Result<u64> {
        if n == 0 {
            return Ok(0);
        }
        ensure!(
            n <= 64,
            "BitReader: read_bits count must be 0..=64, got {n}"
        );

        let mut value: u64 = 0;
        for _ in 0..n {
            value = (value << 1) | self.read_bit()? as u64;
        }
        Ok(value)
    }

    /// Read an unsigned Exp-Golomb coded value: ue(v).
    ///
    /// Format: leading_zeros | 1 | suffix_bits
    /// Value = (1 << leading_zeros) - 1 + read_bits(leading_zeros)
    pub fn read_ue(&mut self) -> Result<u32> {
        let mut leading_zeros: u32 = 0;
        loop {
            let bit = self.read_bit()?;
            if bit == 1 {
                break;
            }
            leading_zeros += 1;
            ensure!(
                leading_zeros <= 31,
                "BitReader: exp-golomb leading zeros exceed 31"
            );
        }

        if leading_zeros == 0 {
            return Ok(0);
        }

        let suffix = self.read_bits(leading_zeros as u8)? as u32;
        Ok((1u32 << leading_zeros) - 1 + suffix)
    }

    /// Read a signed Exp-Golomb coded value: se(v).
    ///
    /// Mapping: ue=0 -> 0, ue=1 -> 1, ue=2 -> -1, ue=3 -> 2, ue=4 -> -2, ...
    /// se = ceil(ue/2) * (-1)^(ue+1)
    pub fn read_se(&mut self) -> Result<i32> {
        let ue = self.read_ue()?;
        let abs_val = ((ue + 1) >> 1) as i32;
        if ue & 1 == 0 {
            Ok(-abs_val)
        } else {
            Ok(abs_val)
        }
    }

    /// Skip `n` bits.
    pub fn skip_bits(&mut self, n: usize) -> Result<()> {
        if n > self.bits_remaining() {
            bail!(
                "BitReader: cannot skip {} bits, only {} remain",
                n,
                self.bits_remaining()
            );
        }
        for _ in 0..n {
            self.read_bit()?;
        }
        Ok(())
    }

    /// Advance to the next byte boundary. If already aligned, does nothing.
    #[cfg(test)]
    pub fn byte_alignment(&mut self) -> Result<()> {
        if self.bit_pos != 0 {
            let remaining = 8 - self.bit_pos as usize;
            self.skip_bits(remaining)?;
        }
        Ok(())
    }

    /// Read a single byte (must be byte-aligned).
    #[cfg(test)]
    pub fn read_byte(&mut self) -> Result<u8> {
        ensure!(
            self.bit_pos == 0,
            "BitReader: read_byte called when not byte-aligned"
        );
        self.read_bits(8).map(|v| v as u8)
    }

    /// Peek at the next bit without advancing the reader.
    #[cfg(test)]
    pub fn peek_bit(&self) -> Result<u8> {
        let mut snapshot = Self {
            data: self.data,
            byte_pos: self.byte_pos,
            bit_pos: self.bit_pos,
            num_zeros: self.num_zeros,
            skip_ep: self.skip_ep,
        };
        snapshot.read_bit()
    }

    /// Read a flag (single bit) as a boolean.
    pub fn read_flag(&mut self) -> Result<bool> {
        Ok(self.read_bit()? != 0)
    }

    /// Consume this BitReader and return the underlying data slice starting
    /// from the current byte position. Must be byte-aligned.
    #[cfg(test)]
    pub fn into_inner(self) -> Result<&'a [u8]> {
        ensure!(
            self.bit_pos == 0,
            "BitReader: into_inner called when not byte-aligned"
        );
        if self.byte_pos >= self.data.len() {
            Ok(&[])
        } else {
            Ok(&self.data[self.byte_pos..])
        }
    }
}

// ---------------------------------------------------------------------------
// CABAC context model
// ---------------------------------------------------------------------------

/// A single CABAC context model: probability state index + most probable symbol.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub struct ContextModel {
    /// Probability state index (0..63). Higher values mean MPS is more likely.
    pub state_idx: u8,
    /// Most probable symbol: 0 or 1.
    pub mps: u8,
}

#[cfg(test)]
impl ContextModel {
    /// Initialize a context model from the HEVC init parameters.
    ///
    /// Per ITU-T H.265, Section 9.3.2.6:
    ///   slopeIdx  = init_value >> 4
    ///   offsetIdx = init_value & 15
    ///   m = slopeIdx * 5 - 45
    ///   n = (offsetIdx << 3) - 16
    ///   preCtxState = Clip3(1, 126, ((m * Clip3(0, 51, SliceQpY)) >> 4) + n)
    ///   if preCtxState <= 63: pStateIdx = 63 - preCtxState, valMps = 0
    ///   else:                 pStateIdx = preCtxState - 64,  valMps = 1
    pub fn from_init_value(init_value: u8, slice_qp: i32) -> Self {
        let slope_idx = (init_value >> 4) as i32;
        let offset_idx = (init_value & 15) as i32;
        let m = slope_idx * 5 - 45;
        let n = (offset_idx << 3) - 16;

        let clamped_qp = slice_qp.clamp(0, 51);
        let pre_ctx_state = (((m * clamped_qp) >> 4) + n).clamp(1, 126);

        if pre_ctx_state <= 63 {
            Self {
                state_idx: (63 - pre_ctx_state) as u8,
                mps: 0,
            }
        } else {
            Self {
                state_idx: (pre_ctx_state - 64) as u8,
                mps: 1,
            }
        }
    }

    /// Create a context in its equi-probable initial state (state 0, mps 0).
    pub fn equiprobable() -> Self {
        Self {
            state_idx: 0,
            mps: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// CabacReader -- HEVC arithmetic decoding engine
// ---------------------------------------------------------------------------

/// HEVC CABAC (Context-based Adaptive Binary Arithmetic Coding) decoder.
///
/// Implements the arithmetic decoding process specified in ITU-T H.265,
/// Section 9.3.3. Uses 9-bit precision for the interval range as required
/// by the spec.
#[cfg(test)]
pub struct CabacReader<'a> {
    data: &'a [u8],
    /// Current byte position in the data stream.
    byte_pos: usize,
    /// Bits consumed from the current byte (0..8).
    bits_consumed: u8,
    /// Current interval range (9-bit, value in [256..510] after renormalization).
    ivl_curr_range: u16,
    /// Current interval offset (9-bit, value in [0..ivl_curr_range)).
    ivl_offset: u16,
    /// Context models, indexed by context index.
    contexts: Vec<ContextModel>,
}

#[cfg(test)]
impl<'a> CabacReader<'a> {
    /// Initialize the CABAC engine from a BitReader.
    ///
    /// Consumes the BitReader (which should be positioned right after the
    /// slice header) and sets up the arithmetic decoder state per
    /// ITU-T H.265, Section 9.3.2.2.
    pub fn from_bit_reader(
        reader: BitReader<'a>,
        init_values: &[u8],
        slice_qp: i32,
    ) -> Result<Self> {
        let data = reader.into_inner()?;

        let contexts: Vec<ContextModel> = init_values
            .iter()
            .map(|&iv| ContextModel::from_init_value(iv, slice_qp))
            .collect();

        let mut cabac = Self {
            data,
            byte_pos: 0,
            bits_consumed: 0,
            ivl_curr_range: 510,
            ivl_offset: 0,
            contexts,
        };

        cabac.ivl_offset = cabac.read_raw_bits(9)? as u16;

        Ok(cabac)
    }

    /// Initialize the CABAC engine directly from a byte slice.
    ///
    /// The slice must start at the first byte of CABAC-coded data
    /// (immediately after the slice header byte-alignment).
    pub fn from_bytes(
        data: &'a [u8],
        init_values: &[u8],
        slice_qp: i32,
    ) -> Result<Self> {
        let contexts: Vec<ContextModel> = init_values
            .iter()
            .map(|&iv| ContextModel::from_init_value(iv, slice_qp))
            .collect();

        let mut cabac = Self {
            data,
            byte_pos: 0,
            bits_consumed: 0,
            ivl_curr_range: 510,
            ivl_offset: 0,
            contexts,
        };

        cabac.ivl_offset = cabac.read_raw_bits(9)? as u16;

        Ok(cabac)
    }

    /// Read `n` raw bits from the bitstream (no emulation prevention --
    /// the input is assumed to already be RBSP).
    fn read_raw_bits(&mut self, n: u32) -> Result<u32> {
        let mut value: u32 = 0;
        for _ in 0..n {
            value = (value << 1) | self.read_raw_bit()? as u32;
        }
        Ok(value)
    }

    /// Read a single raw bit from the bitstream.
    fn read_raw_bit(&mut self) -> Result<u8> {
        ensure!(
            self.byte_pos < self.data.len(),
            "CabacReader: read past end of data"
        );
        let bit = (self.data[self.byte_pos] >> (7 - self.bits_consumed)) & 1;
        self.bits_consumed += 1;
        if self.bits_consumed == 8 {
            self.bits_consumed = 0;
            self.byte_pos += 1;
        }
        Ok(bit)
    }

    /// Renormalize the arithmetic decoder state.
    ///
    /// Per ITU-T H.265, Section 9.3.3.2.3:
    /// While ivlCurrRange < 256, shift both range and offset left by 1,
    /// reading one new bit into the LSB of ivlOffset.
    fn renormalize(&mut self) -> Result<()> {
        while self.ivl_curr_range < 256 {
            self.ivl_curr_range <<= 1;
            self.ivl_offset <<= 1;
            self.ivl_offset |= self.read_raw_bit()? as u16;
        }
        Ok(())
    }

    /// Mutable access to a context model by index.
    pub fn context_mut(&mut self, ctx_idx: usize) -> Result<&mut ContextModel> {
        if ctx_idx >= self.contexts.len() {
            bail!(
                "CabacReader: context index {ctx_idx} out of range (max {})",
                self.contexts.len()
            );
        }
        Ok(&mut self.contexts[ctx_idx])
    }

    /// Read-only access to a context model by index.
    pub fn context(&self, ctx_idx: usize) -> Result<&ContextModel> {
        if ctx_idx >= self.contexts.len() {
            bail!(
                "CabacReader: context index {ctx_idx} out of range (max {})",
                self.contexts.len()
            );
        }
        Ok(&self.contexts[ctx_idx])
    }

    /// Decode a single bin (binary decision) using the specified context.
    ///
    /// Implements ITU-T H.265, Section 9.3.3.2 (DecodeDecision).
    pub fn decode_decision(&mut self, ctx_idx: usize) -> Result<u8> {
        if ctx_idx >= self.contexts.len() {
            bail!(
                "CabacReader: context index {ctx_idx} out of range (max {})",
                self.contexts.len()
            );
        }

        let state_idx = self.contexts[ctx_idx].state_idx as usize;
        let mps = self.contexts[ctx_idx].mps;

        let q_range_idx = ((self.ivl_curr_range >> 6) & 3) as usize;
        let ivl_lps_range = RANGE_TAB_LPS[state_idx][q_range_idx];

        self.ivl_curr_range -= ivl_lps_range;

        let bin_val;
        if self.ivl_offset >= self.ivl_curr_range {
            // Decoded the LPS
            bin_val = 1 - mps;
            self.ivl_offset -= self.ivl_curr_range;
            self.ivl_curr_range = ivl_lps_range;

            if state_idx == 0 {
                self.contexts[ctx_idx].mps = 1 - mps;
            }
            self.contexts[ctx_idx].state_idx = TRANS_IDX_LPS[state_idx];
        } else {
            // Decoded the MPS
            bin_val = mps;
            self.contexts[ctx_idx].state_idx = TRANS_IDX_MPS[state_idx];
        }

        self.renormalize()?;
        Ok(bin_val)
    }

    /// Decode a single bin in bypass mode (equiprobable, no context).
    ///
    /// Implements ITU-T H.265, Section 9.3.3.2.3 (DecodeBypass).
    pub fn decode_bypass(&mut self) -> Result<u8> {
        self.ivl_offset <<= 1;
        self.ivl_offset |= self.read_raw_bit()? as u16;

        if self.ivl_offset >= self.ivl_curr_range {
            self.ivl_offset -= self.ivl_curr_range;
            Ok(1)
        } else {
            Ok(0)
        }
    }

    /// Decode multiple bypass bins and return them as a right-aligned integer.
    pub fn decode_bypass_bins(&mut self, count: u32) -> Result<u32> {
        let mut value: u32 = 0;
        for _ in 0..count {
            value = (value << 1) | self.decode_bypass()? as u32;
        }
        Ok(value)
    }

    /// Decode the terminating bin (end-of-slice or end-of-sub-stream).
    ///
    /// Implements ITU-T H.265, Section 9.3.3.2.4 (DecodeTerminate).
    /// Returns 1 if the sub-stream is terminated, 0 otherwise.
    pub fn decode_terminate(&mut self) -> Result<u8> {
        self.ivl_curr_range -= 2;

        if self.ivl_offset >= self.ivl_curr_range {
            Ok(1)
        } else {
            self.renormalize()?;
            Ok(0)
        }
    }

    /// Decode a unary-coded value using the given context index.
    /// Reads bins until a 0 is decoded; the value is the count of 1s.
    pub fn decode_unary(&mut self, ctx_idx: usize) -> Result<u32> {
        let mut value = 0u32;
        while self.decode_decision(ctx_idx)? == 1 {
            value += 1;
        }
        Ok(value)
    }

    /// Decode a truncated unary value with a maximum.
    /// Each bin uses the same context. Returns the count of 1s read,
    /// up to `max` (where the last 1 at position `max` has no trailing 0).
    pub fn decode_truncated_unary(&mut self, ctx_idx: usize, max: u32) -> Result<u32> {
        let mut value = 0u32;
        while value < max {
            if self.decode_decision(ctx_idx)? == 0 {
                break;
            }
            value += 1;
        }
        Ok(value)
    }

    /// Decode a truncated unary value where each bin position may use a
    /// different context. `ctx_indices` provides the context index for each
    /// bin position. If more bins are needed than entries in `ctx_indices`,
    /// the last entry is reused.
    pub fn decode_truncated_unary_multi_ctx(
        &mut self,
        ctx_indices: &[usize],
        max: u32,
    ) -> Result<u32> {
        if ctx_indices.is_empty() {
            bail!("CabacReader: empty context index list");
        }
        let last_ctx = ctx_indices[ctx_indices.len() - 1];
        let mut value = 0u32;
        while value < max {
            let idx = if (value as usize) < ctx_indices.len() {
                ctx_indices[value as usize]
            } else {
                last_ctx
            };
            if self.decode_decision(idx)? == 0 {
                break;
            }
            value += 1;
        }
        Ok(value)
    }

    /// Reset context models (e.g., at the start of a new slice/tile).
    pub fn reset_contexts(&mut self, init_values: &[u8], slice_qp: i32) {
        self.contexts = init_values
            .iter()
            .map(|&iv| ContextModel::from_init_value(iv, slice_qp))
            .collect();
    }

    /// Returns the number of context models currently initialized.
    pub fn num_contexts(&self) -> usize {
        self.contexts.len()
    }

    /// Returns the current interval range (for debugging/validation).
    pub fn current_range(&self) -> u16 {
        self.ivl_curr_range
    }

    /// Returns the current interval offset (for debugging/validation).
    pub fn current_offset(&self) -> u16 {
        self.ivl_offset
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- BitReader tests --

    #[test]
    fn read_individual_bits() {
        let data = [0b1010_0110];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert!(r.read_bit().is_err());
    }

    #[test]
    fn read_multi_bit_fields() {
        let data = [0b1101_0011, 0b1000_0000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(4).unwrap(), 0b1101);
        assert_eq!(r.read_bits(4).unwrap(), 0b0011);
        assert_eq!(r.read_bits(1).unwrap(), 1);
        assert_eq!(r.bits_remaining(), 7);
    }

    #[test]
    fn read_bits_cross_byte() {
        let data = [0xFF, 0x0F];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(12).unwrap(), 0xFF0);
        assert_eq!(r.read_bits(4).unwrap(), 0x0F);
    }

    #[test]
    fn emulation_prevention_basic() {
        // 0x00 0x00 0x03 0x01 -> 0x00 0x00 0x01 (0x03 stripped)
        let data = [0x00, 0x00, 0x03, 0x01];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(8).unwrap(), 0x00);
        assert_eq!(r.read_bits(8).unwrap(), 0x00);
        assert_eq!(r.read_bits(8).unwrap(), 0x01);
    }

    #[test]
    fn emulation_prevention_multiple() {
        let data = [0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x02];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(8).unwrap(), 0x00);
        assert_eq!(r.read_bits(8).unwrap(), 0x00);
        assert_eq!(r.read_bits(8).unwrap(), 0x00);
        assert_eq!(r.read_bits(8).unwrap(), 0x00);
        assert_eq!(r.read_bits(8).unwrap(), 0x02);
    }

    #[test]
    fn no_false_emulation_prevention() {
        // 0x00 0x01 0x03 -- only two consecutive 0x00 trigger stripping
        let data = [0x00, 0x01, 0x03];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bits(8).unwrap(), 0x00);
        assert_eq!(r.read_bits(8).unwrap(), 0x01);
        assert_eq!(r.read_bits(8).unwrap(), 0x03);
    }

    #[test]
    fn read_exp_golomb_unsigned() {
        // ue: 0 -> "1", 1 -> "010", 2 -> "011", 3 -> "00100", 7 -> "00010 00"
        let data = [0b1_010_011_0, 0b0100_0001, 0b0_00_00000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_ue().unwrap(), 0);
        assert_eq!(r.read_ue().unwrap(), 1);
        assert_eq!(r.read_ue().unwrap(), 2);
        assert_eq!(r.read_ue().unwrap(), 3);
        assert_eq!(r.read_ue().unwrap(), 7);
    }

    #[test]
    fn read_exp_golomb_signed() {
        // se: 0 -> 0, 1 -> 1, 2 -> -1, 3 -> 2, 4 -> -2
        let data = [0b1_010_011_0, 0b0100_0010, 0b1_0000000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_se().unwrap(), 0);
        assert_eq!(r.read_se().unwrap(), 1);
        assert_eq!(r.read_se().unwrap(), -1);
        assert_eq!(r.read_se().unwrap(), 2);
        assert_eq!(r.read_se().unwrap(), -2);
    }

    #[test]
    fn byte_alignment_from_mid_byte() {
        let data = [0xFF, 0xAA];
        let mut r = BitReader::new(&data);
        r.read_bits(3).unwrap();
        r.byte_alignment().unwrap();
        assert_eq!(r.read_bits(8).unwrap(), 0xAA);
    }

    #[test]
    fn byte_alignment_already_aligned() {
        let data = [0xFF, 0xAA];
        let mut r = BitReader::new(&data);
        r.read_bits(8).unwrap();
        r.byte_alignment().unwrap();
        assert_eq!(r.read_bits(8).unwrap(), 0xAA);
    }

    #[test]
    fn skip_bits_advances_correctly() {
        let data = [0xFF, 0x00, 0xAB];
        let mut r = BitReader::new(&data);
        r.skip_bits(12).unwrap();
        assert_eq!(r.read_bits(4).unwrap(), 0x0);
        assert_eq!(r.read_bits(8).unwrap(), 0xAB);
    }

    #[test]
    fn read_flag() {
        let data = [0b1000_0000];
        let mut r = BitReader::new(&data);
        assert!(r.read_flag().unwrap());
        assert!(!r.read_flag().unwrap());
    }

    #[test]
    fn bits_remaining_tracking() {
        let data = [0xFF, 0xAA];
        let mut r = BitReader::new(&data);
        assert_eq!(r.bits_remaining(), 16);
        r.read_bits(5).unwrap();
        assert_eq!(r.bits_remaining(), 11);
        r.read_bits(11).unwrap();
        assert_eq!(r.bits_remaining(), 0);
        assert!(r.is_empty());
    }

    #[test]
    fn peek_bit_does_not_advance() {
        let data = [0xA0]; // 1010_0000
        let r = BitReader::new(&data);
        assert_eq!(r.peek_bit().unwrap(), 1);
        assert_eq!(r.peek_bit().unwrap(), 1);
    }

    #[test]
    fn into_inner_returns_remaining() {
        let data = [0xFF, 0xAA, 0xBB];
        let mut r = BitReader::new(&data);
        r.read_bits(8).unwrap();
        let remaining = r.into_inner().unwrap();
        assert_eq!(remaining, &[0xAA, 0xBB]);
    }

    #[test]
    fn into_inner_empty_at_end() {
        let data = [0xFF];
        let mut r = BitReader::new(&data);
        r.read_bits(8).unwrap();
        let remaining = r.into_inner().unwrap();
        assert!(remaining.is_empty());
    }

    #[test]
    fn into_inner_not_aligned_error() {
        let data = [0xFF];
        let mut r = BitReader::new(&data);
        r.read_bits(3).unwrap();
        assert!(r.into_inner().is_err());
    }

    #[test]
    fn read_byte_aligned() {
        let data = [0xDE, 0xAD];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_byte().unwrap(), 0xDE);
        assert_eq!(r.read_byte().unwrap(), 0xAD);
    }

    #[test]
    fn read_byte_unaligned_error() {
        let data = [0xFF];
        let mut r = BitReader::new(&data);
        r.read_bit().unwrap();
        assert!(r.read_byte().is_err());
    }

    #[test]
    fn error_past_end() {
        let data = [0xFF];
        let mut r = BitReader::new(&data);
        r.read_bits(8).unwrap();
        assert!(r.read_bit().is_err());
    }

    #[test]
    fn underflow_on_read_bits() {
        let data = [0xFF];
        let mut r = BitReader::new(&data);
        // 9 bits from a 1-byte buffer: should fail at bit 9
        assert!(r.read_bits(9).is_err());
    }

    #[test]
    fn underflow_on_skip() {
        let data = [0xFF];
        let mut r = BitReader::new(&data);
        assert!(r.skip_bits(9).is_err());
    }

    #[test]
    fn exp_golomb_large_value() {
        // ue(7): 3 leading zeros, then 1, then 3 suffix bits = 000
        // Binary: 0001000
        let data = [0b00010000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_ue().unwrap(), 7);
    }

    // -- ContextModel tests --

    #[test]
    fn context_model_init_low() {
        let ctx = ContextModel::from_init_value(0, 26);
        assert_eq!(ctx.mps, 0);
        assert!(ctx.state_idx <= 63);
    }

    #[test]
    fn context_model_init_high() {
        // init_value=0xFF: slope=15, offset=15
        // m=15*5-45=30, n=15*8-16=104
        // preCtxState = clip(1,126, (30*26)>>4 + 104) = clip(1,126, 48+104) = 126
        // > 63 -> state=126-64=62, mps=1
        let ctx = ContextModel::from_init_value(0xFF, 26);
        assert_eq!(ctx.mps, 1);
        assert_eq!(ctx.state_idx, 62);
    }

    #[test]
    fn context_model_equiprobable() {
        let ctx = ContextModel::equiprobable();
        assert_eq!(ctx.state_idx, 0);
        assert_eq!(ctx.mps, 0);
    }

    #[test]
    fn context_model_qp_clamping() {
        let ctx_low = ContextModel::from_init_value(154, -10);
        let ctx_zero = ContextModel::from_init_value(154, 0);
        assert_eq!(ctx_low.state_idx, ctx_zero.state_idx);
        assert_eq!(ctx_low.mps, ctx_zero.mps);

        let ctx_high = ContextModel::from_init_value(154, 100);
        let ctx_51 = ContextModel::from_init_value(154, 51);
        assert_eq!(ctx_high.state_idx, ctx_51.state_idx);
        assert_eq!(ctx_high.mps, ctx_51.mps);
    }

    // -- CabacReader tests --

    #[test]
    fn cabac_initialization() {
        let data = [0xFF, 0xFF, 0xFF, 0xFF];
        let init_values = [154, 154, 154];
        let cabac = CabacReader::from_bytes(&data, &init_values, 26).unwrap();

        // 9 bits of 0xFF 0xFF = 0b1_1111_1111 = 511
        assert_eq!(cabac.current_range(), 510);
        assert_eq!(cabac.current_offset(), 0x1FF);
        assert_eq!(cabac.num_contexts(), 3);
    }

    #[test]
    fn cabac_from_bit_reader() {
        let data = [0xAA, 0xFF, 0xFF, 0xFF];
        let mut reader = BitReader::new(&data);
        reader.read_bits(8).unwrap(); // simulate slice header

        let init_values = [154];
        let cabac = CabacReader::from_bit_reader(reader, &init_values, 26).unwrap();
        assert_eq!(cabac.current_range(), 510);
        assert_eq!(cabac.num_contexts(), 1);
    }

    #[test]
    fn cabac_decode_bypass_all_zeros() {
        let data = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let mut cabac = CabacReader::from_bytes(&data, &[], 26).unwrap();
        // All-zero input: offset stays 0, always < range -> 0
        assert_eq!(cabac.decode_bypass().unwrap(), 0);
        assert_eq!(cabac.decode_bypass().unwrap(), 0);
    }

    #[test]
    fn cabac_decode_bypass_bins() {
        let data = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let mut cabac = CabacReader::from_bytes(&data, &[], 26).unwrap();
        assert_eq!(cabac.decode_bypass_bins(4).unwrap(), 0);
    }

    #[test]
    fn cabac_decode_terminate_not_terminated() {
        let data = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let mut cabac = CabacReader::from_bytes(&data, &[], 26).unwrap();
        assert_eq!(cabac.decode_terminate().unwrap(), 0);
    }

    #[test]
    fn cabac_decode_terminate_terminated() {
        // Need ivlOffset >= range-2. Init range=510 -> range-2=508.
        // 9 bits of 0xFF 0x80 = 0b1_1111_1111 = 511, 511 >= 508 -> terminated.
        let data = [0xFF, 0x80, 0x00, 0x00];
        let mut cabac = CabacReader::from_bytes(&data, &[], 26).unwrap();
        assert_eq!(cabac.decode_terminate().unwrap(), 1);
    }

    #[test]
    fn cabac_decode_decision_smoke() {
        let data = [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01,
                     0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01];
        let init_values = [154];
        let mut cabac = CabacReader::from_bytes(&data, &init_values, 26).unwrap();

        for _ in 0..10 {
            let bin = cabac.decode_decision(0).unwrap();
            assert!(bin <= 1);
        }
    }

    #[test]
    fn cabac_reset_contexts() {
        let data = [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01];
        let init_values = [154];
        let mut cabac = CabacReader::from_bytes(&data, &init_values, 26).unwrap();

        cabac.decode_decision(0).unwrap();

        cabac.reset_contexts(&[154, 110], 30);
        assert_eq!(cabac.num_contexts(), 2);
    }

    #[test]
    fn cabac_context_access() {
        let data = [0xFF, 0xFF, 0xFF, 0xFF];
        let init_values = [154, 110];
        let cabac = CabacReader::from_bytes(&data, &init_values, 26).unwrap();
        assert!(cabac.context(0).is_ok());
        assert!(cabac.context(1).is_ok());
        assert!(cabac.context(2).is_err());
    }

    #[test]
    fn cabac_context_mut_access() {
        let data = [0xFF, 0xFF, 0xFF, 0xFF];
        let init_values = [154];
        let mut cabac = CabacReader::from_bytes(&data, &init_values, 26).unwrap();
        {
            let ctx = cabac.context_mut(0).unwrap();
            ctx.state_idx = 10;
            ctx.mps = 1;
        }
        let ctx = cabac.context(0).unwrap();
        assert_eq!(ctx.state_idx, 10);
        assert_eq!(ctx.mps, 1);
    }

    // -- Transition table validation --

    #[test]
    fn transition_table_bounds() {
        for &next in TRANS_IDX_LPS.iter() {
            assert!(next <= 63, "LPS transition out of range: {next}");
        }
        for &next in TRANS_IDX_MPS.iter() {
            assert!(next <= 63, "MPS transition out of range: {next}");
        }
    }

    #[test]
    fn range_tab_lps_bounds() {
        for row in RANGE_TAB_LPS.iter() {
            for &val in row.iter() {
                assert!(
                    val >= 2 && val <= 255,
                    "LPS range value out of bounds: {val}"
                );
            }
        }
    }

    #[test]
    fn transition_table_state_63_lps() {
        assert_eq!(TRANS_IDX_LPS[63], 63);
    }

    #[test]
    fn transition_table_state_62_mps() {
        assert_eq!(TRANS_IDX_MPS[62], 62);
    }
}
