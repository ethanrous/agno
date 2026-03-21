/// VP8 DC quantization factors indexed by q_index (0..=127).
#[rustfmt::skip]
pub const DC_QUANT: [i16; 128] = [
    4,5,6,7,8,9,10,10,11,12,13,14,15,16,17,17,
    18,19,20,20,21,21,22,22,23,23,24,25,25,26,27,28,
    29,30,31,32,33,34,35,36,37,37,38,39,40,41,42,43,
    44,45,46,46,47,48,49,50,51,52,53,54,55,56,57,58,
    59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,
    75,76,76,77,78,79,80,81,82,83,84,85,86,87,88,89,
    91,93,95,96,98,100,101,102,104,106,108,110,112,114,116,118,
    122,124,126,128,130,132,134,136,138,140,143,145,148,151,154,157,
];

/// VP8 AC quantization factors indexed by q_index (0..=127).
#[rustfmt::skip]
pub const AC_QUANT: [i16; 128] = [
    4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,
    20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,
    36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,
    52,53,54,55,56,57,58,59,60,62,64,66,68,70,72,74,
    76,78,80,82,84,86,88,90,92,94,96,98,100,102,104,106,
    108,110,112,114,116,119,122,125,128,131,134,137,140,143,145,148,
    151,154,157,160,163,166,169,172,175,178,181,184,187,190,193,196,
    199,202,205,208,211,214,217,220,224,227,230,233,236,239,242,245,
];

/// Map quality (0-100) to VP8 q_index (0-127).
/// Quality 100 => q_index 0 (best), quality 0 => q_index 127 (worst).
pub fn quality_to_qindex(quality: u8) -> u8 {
    let q = quality.min(100) as u32;
    (((100 - q) * 127 + 50) / 100).min(127) as u8
}

/// VP8 quantizer with per-component factors derived from a single q_index.
pub struct VpxQuantizer {
    pub y1_dc: i16,
    pub y1_ac: i16,
    pub y2_dc: i16,
    pub y2_ac: i16,
    pub uv_dc: i16,
    pub uv_ac: i16,
}

impl VpxQuantizer {
    pub fn new(q_index: u8) -> Self {
        let qi = q_index as usize;
        Self {
            y1_dc: DC_QUANT[qi],
            y1_ac: AC_QUANT[qi],
            y2_dc: DC_QUANT[qi] * 2,
            y2_ac: (AC_QUANT[qi] as i32 * 155 / 100).max(8) as i16,
            uv_dc: DC_QUANT[qi.min(117)],
            uv_ac: AC_QUANT[qi],
        }
    }
}

/// Quantize a 4x4 coefficient block: divide each coefficient by `quant`
/// with rounding towards the nearest integer.
pub fn quantize_block(coeffs: &[i16; 16], dc_quant: i16, ac_quant: i16) -> [i16; 16] {
    let mut out = [0i16; 16];
    // DC coefficient (index 0) uses dc_quant
    let dq = dc_quant.max(1) as i32;
    let c0 = coeffs[0] as i32;
    out[0] = if c0 >= 0 {
        ((c0 + dq / 2) / dq) as i16
    } else {
        ((c0 - dq / 2) / dq) as i16
    };
    // AC coefficients (indices 1..15) use ac_quant
    let aq = ac_quant.max(1) as i32;
    for i in 1..16 {
        let c = coeffs[i] as i32;
        out[i] = if c >= 0 {
            ((c + aq / 2) / aq) as i16
        } else {
            ((c - aq / 2) / aq) as i16
        };
    }
    out
}

/// Quantize a 4x4 block uniformly (all coefficients use the same quant factor).
/// Used for Y2 (WHT) blocks.
pub fn quantize_block_uniform(coeffs: &[i16; 16], quant: i16) -> [i16; 16] {
    let mut out = [0i16; 16];
    let q = quant.max(1) as i32;
    for i in 0..16 {
        let c = coeffs[i] as i32;
        out[i] = if c >= 0 {
            ((c + q / 2) / q) as i16
        } else {
            ((c - q / 2) / q) as i16
        };
    }
    out
}
