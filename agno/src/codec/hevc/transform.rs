// HEVC (H.265) inverse quantization and inverse transform.
//
// Implements ITU-T H.265 Section 8.6.2 (scaling/inverse quantization) and
// Section 8.6.4 (inverse transform). Supports all HEVC transform block sizes
// (4x4, 8x8, 16x16, 32x32) with both DCT and DST kernels.

// -- Quantization tables (Table 8-11) --

/// Scaling factors indexed by qp % 6 (Table 8-11 of H.265).
const LEVEL_SCALE: [i32; 6] = [40, 45, 51, 57, 64, 72];

// -- DCT/DST matrix coefficients from H.265 Tables 8-2 through 8-6 --

/// 4x4 DST-VII matrix (Table 8-2), used for 4x4 luma intra TUs.
const DST4: [[i32; 4]; 4] = [
    [29, 55, 74, 84],
    [74, 74, 0, -74],
    [84, -29, -74, 55],
    [55, -84, 74, -29],
];

/// 4x4 DCT-II matrix (Table 8-3).
const DCT4: [[i32; 4]; 4] = [
    [64, 64, 64, 64],
    [83, 36, -36, -83],
    [64, -64, -64, 64],
    [36, -83, 83, -36],
];

/// 8-point odd-indexed DCT coefficients (Table 8-4).
/// Row k contains coefficients for the (2k+1)-th basis vector.
const DCT8_ODD: [[i32; 4]; 4] = [
    [89, 75, 50, 18],
    [75, -18, -89, -50],
    [50, -89, 18, 75],
    [18, -50, 75, -89],
];

/// 16-point odd-indexed DCT coefficients (Table 8-5).
/// Row k contains coefficients for the (2k+1)-th basis vector.
const DCT16_ODD: [[i32; 8]; 8] = [
    [90, 87, 80, 70, 57, 43, 25, 9],
    [87, 57, 9, -43, -80, -90, -70, -25],
    [80, 9, -70, -87, -25, 57, 90, 43],
    [70, -43, -87, 9, 90, 25, -80, -57],
    [57, -80, -25, 90, -9, -87, 43, 70],
    [43, -90, 57, 25, -87, 70, 9, -80],
    [25, -70, 90, -80, 43, 9, -57, 87],
    [9, -25, 43, -57, 70, -80, 87, -90],
];

/// 32-point odd-indexed DCT coefficients (Table 8-6).
/// Row k contains coefficients for the (2k+1)-th basis vector.
const DCT32_ODD: [[i32; 16]; 16] = [
    [90, 90, 88, 85, 82, 78, 73, 67, 61, 54, 46, 38, 31, 22, 13, 4],
    [90, 82, 67, 46, 22, -4, -31, -54, -73, -85, -90, -88, -78, -61, -38, -13],
    [88, 67, 31, -13, -54, -82, -90, -78, -46, -4, 38, 73, 90, 85, 61, 22],
    [85, 46, -13, -67, -90, -73, -22, 38, 82, 88, 54, -4, -61, -90, -78, -31],
    [82, 22, -54, -90, -61, 13, 78, 85, 31, -46, -90, -67, 4, 73, 88, 38],
    [78, -4, -82, -73, 13, 85, 67, -22, -88, -61, 31, 90, 54, -38, -90, -46],
    [73, -31, -90, -22, 78, 67, -38, -90, -13, 82, 61, -46, -88, -4, 85, 54],
    [67, -54, -78, 38, 85, -22, -90, 4, 90, 13, -88, -31, 82, 46, -73, -61],
    [61, -73, -46, 82, 31, -88, -13, 90, -4, -90, 22, 85, -38, -78, 54, 67],
    [54, -85, -4, 88, -46, -61, 82, 13, -90, 38, 67, -78, -22, 90, -31, -73],
    [46, -90, 38, 54, -90, 31, 61, -88, 22, 67, -85, 13, 73, -82, 4, 78],
    [38, -88, 73, -4, -67, 90, -46, -31, 85, -78, 13, 61, -90, 54, 22, -82],
    [31, -78, 90, -61, 4, 54, -88, 82, -38, -22, 73, -90, 67, -13, -46, 85],
    [22, -61, 85, -90, 73, -38, -4, 46, -78, 90, -82, 54, -13, -31, 67, -88],
    [13, -38, 61, -78, 88, -90, 85, -73, 54, -31, 4, 22, -46, 67, -82, 90],
    [4, -13, 22, -31, 38, -46, 54, -61, 67, -73, 78, -82, 85, -88, 90, -90],
];

/// Clip a value to the signed 16-bit range [-32768, 32767].
#[inline(always)]
fn clip_i16(x: i32) -> i32 {
    x.clamp(-32768, 32767)
}

/// Clip a value to the valid residual range for a given bit depth.
#[inline(always)]
fn clip_residual(x: i32, bit_depth: u8) -> i32 {
    let max = (1i32 << (bit_depth - 1)) - 1;
    let min = -(1i32 << (bit_depth - 1));
    x.clamp(min, max)
}

// -- Inverse Quantization (Section 8.6.2 / 8.6.3) --

/// Inverse quantize (scale) transform coefficients in-place.
///
/// Applies the HEVC scaling process from Section 8.6.3 with the flat
/// (default) scaling matrix value of 16. After this call, `coeffs` contains
/// the scaled transform coefficients ready for inverse transform.
///
/// # Parameters
/// - `coeffs`: transform coefficient levels; modified in-place to scaled values
/// - `qp`: quantization parameter (0..51 typical, can exceed for high bit depth)
/// - `bit_depth`: luma or chroma bit depth (typically 8 or 10)
/// - `log2_size`: log2 of the transform block width (2 for 4x4, 3 for 8x8, etc.)
pub fn dequantize(coeffs: &mut [i32], qp: i32, bit_depth: u8, log2_size: u32) {
    // Clamp QP to valid range (0..51 + bit_depth extensions) to prevent shift overflow
    let qp_clamped = qp.clamp(0, 51 + 6 * (bit_depth as i32 - 8));
    let qp_per = qp_clamped / 6;
    let qp_rem = (qp_clamped % 6) as usize;
    let scale = LEVEL_SCALE[qp_rem];

    // Default (flat) scaling matrix value per spec.
    const DEFAULT_SCALE_M: i32 = 16;

    // bdShift = bit_depth + log2_size + 10 - (bit_depth used in transform)
    // Per spec Section 8.6.3:
    //   bdShift = bit_depth + Log2(nTbS) - 5
    // This shift normalizes the product of level * m * levelScale after the
    // qp_per left-shift to the correct fixed-point range.
    let bd_shift = bit_depth as i32 + log2_size as i32 - 5;

    // Coefficient range limits for clipping.
    let coeff_min = -(1i32 << 15);
    let coeff_max = (1i32 << 15) - 1;

    if bd_shift >= 0 {
        let add: i64 = if bd_shift == 0 { 0 } else { 1i64 << (bd_shift - 1) };
        for c in coeffs.iter_mut() {
            if *c == 0 {
                continue;
            }
            let scaled = (*c as i64 * DEFAULT_SCALE_M as i64 * scale as i64) << qp_per;
            *c = ((scaled + add) >> bd_shift).clamp(coeff_min as i64, coeff_max as i64) as i32;
        }
    } else {
        let left = (-bd_shift) as u32;
        for c in coeffs.iter_mut() {
            if *c == 0 {
                continue;
            }
            let scaled = (*c as i64 * DEFAULT_SCALE_M as i64 * scale as i64) << qp_per;
            *c = (scaled << left).clamp(coeff_min as i64, coeff_max as i64) as i32;
        }
    }
}

// -- 1-D Transform Kernels --

/// Partial butterfly 4-point inverse DCT (in-place via scratch).
///
/// Reads 4 frequency-domain values from `src` at stride `stride` starting at
/// `offset`, writes 4 spatial-domain values into `dst` at the same positions.
/// `shift` is the right-shift applied after accumulation, with rounding.
fn idct4(src: &[i32], dst: &mut [i32], offset: usize, stride: usize, shift: u32) {
    let add = 1i32 << (shift - 1);

    let s0 = src[offset];
    let s1 = src[offset + stride];
    let s2 = src[offset + 2 * stride];
    let s3 = src[offset + 3 * stride];

    // Even part
    let e0 = DCT4[0][0] * s0 + DCT4[0][1] * s2; // 64*s0 + 64*s2
    let e1 = DCT4[0][0] * s0 - DCT4[0][1] * s2; // 64*s0 - 64*s2

    // Odd part
    let o0 = DCT4[1][0] * s1 + DCT4[1][1] * s3; // 83*s1 + 36*s3
    let o1 = DCT4[1][1] * s1 - DCT4[1][0] * s3; // 36*s1 - 83*s3

    dst[offset] = clip_i16((e0 + o0 + add) >> shift);
    dst[offset + stride] = clip_i16((e1 + o1 + add) >> shift);
    dst[offset + 2 * stride] = clip_i16((e1 - o1 + add) >> shift);
    dst[offset + 3 * stride] = clip_i16((e0 - o0 + add) >> shift);
}

/// Partial butterfly 4-point inverse DST (in-place via scratch).
fn idst4(src: &[i32], dst: &mut [i32], offset: usize, stride: usize, shift: u32) {
    let add = 1i32 << (shift - 1);

    let s0 = src[offset];
    let s1 = src[offset + stride];
    let s2 = src[offset + 2 * stride];
    let s3 = src[offset + 3 * stride];

    // Matrix multiply: out[j] = sum_i DST4[i][j] * s[i]
    // Note: the HEVC DST is applied as a transpose multiply when used as
    // an inverse transform, so we sum over rows for each output column.
    let c0 = DST4[0][0] * s0 + DST4[1][0] * s1 + DST4[2][0] * s2 + DST4[3][0] * s3;
    let c1 = DST4[0][1] * s0 + DST4[1][1] * s1 + DST4[2][1] * s2 + DST4[3][1] * s3;
    let c2 = DST4[0][2] * s0 + DST4[1][2] * s1 + DST4[2][2] * s2 + DST4[3][2] * s3;
    let c3 = DST4[0][3] * s0 + DST4[1][3] * s1 + DST4[2][3] * s2 + DST4[3][3] * s3;

    dst[offset] = clip_i16((c0 + add) >> shift);
    dst[offset + stride] = clip_i16((c1 + add) >> shift);
    dst[offset + 2 * stride] = clip_i16((c2 + add) >> shift);
    dst[offset + 3 * stride] = clip_i16((c3 + add) >> shift);
}

/// Partial butterfly 8-point inverse DCT.
///
/// The 8-point IDCT decomposes into a 4-point even part (reusing the 4x4
/// DCT matrix) and a 4-point odd part (DCT8_ODD). This is the standard
/// HEVC butterfly factorization.
fn idct8(src: &[i32], dst: &mut [i32], offset: usize, stride: usize, shift: u32) {
    let add = 1i32 << (shift - 1);

    // Load all 8 inputs.
    let s: [i32; 8] = [
        src[offset],
        src[offset + stride],
        src[offset + 2 * stride],
        src[offset + 3 * stride],
        src[offset + 4 * stride],
        src[offset + 5 * stride],
        src[offset + 6 * stride],
        src[offset + 7 * stride],
    ];

    // Even part: 4-point DCT on even-indexed inputs [s0, s2, s4, s6].
    let ee0 = DCT4[0][0] * s[0] + DCT4[0][1] * s[4]; // 64*(s0+s4)
    let ee1 = DCT4[0][0] * s[0] - DCT4[0][1] * s[4]; // 64*(s0-s4)
    let eo0 = DCT4[1][0] * s[2] + DCT4[1][1] * s[6]; // 83*s2 + 36*s6
    let eo1 = DCT4[1][1] * s[2] - DCT4[1][0] * s[6]; // 36*s2 - 83*s6

    let e = [
        ee0 + eo0,
        ee1 + eo1,
        ee1 - eo1,
        ee0 - eo0,
    ];

    // Odd part: 4-point transform on odd-indexed inputs [s1, s3, s5, s7].
    let mut o = [0i32; 4];
    for k in 0..4 {
        o[k] = DCT8_ODD[k][0] * s[1]
            + DCT8_ODD[k][1] * s[3]
            + DCT8_ODD[k][2] * s[5]
            + DCT8_ODD[k][3] * s[7];
    }

    // Butterfly combination: output[k] = e[k] + o[k], output[7-k] = e[k] - o[k].
    for k in 0..4 {
        dst[offset + k * stride] = clip_i16((e[k] + o[k] + add) >> shift);
        dst[offset + (7 - k) * stride] = clip_i16((e[k] - o[k] + add) >> shift);
    }
}

/// Partial butterfly 16-point inverse DCT.
///
/// Decomposes into an 8-point even part (recursing through the 8-point even/odd
/// structure) and an 8-point odd part (DCT16_ODD).
fn idct16(src: &[i32], dst: &mut [i32], offset: usize, stride: usize, shift: u32) {
    let add = 1i32 << (shift - 1);

    // Load all 16 inputs.
    let mut s = [0i32; 16];
    for i in 0..16 {
        s[i] = src[offset + i * stride];
    }

    // -- Even part (8-point), operating on even-indexed coefficients --
    // The even part is itself an 8-point IDCT on s[0], s[2], s[4], ..., s[14].

    // Level 2 even-even: 4-point DCT on s[0], s[4], s[8], s[12].
    let eee0 = DCT4[0][0] * s[0] + DCT4[0][1] * s[8];  // 64*(s0 + s8)
    let eee1 = DCT4[0][0] * s[0] - DCT4[0][1] * s[8];  // 64*(s0 - s8)
    let eeo0 = DCT4[1][0] * s[4] + DCT4[1][1] * s[12]; // 83*s4 + 36*s12
    let eeo1 = DCT4[1][1] * s[4] - DCT4[1][0] * s[12]; // 36*s4 - 83*s12

    let ee = [
        eee0 + eeo0,
        eee1 + eeo1,
        eee1 - eeo1,
        eee0 - eeo0,
    ];

    // Level 2 even-odd: 4-point transform on s[2], s[6], s[10], s[14].
    let mut eo = [0i32; 4];
    for k in 0..4 {
        eo[k] = DCT8_ODD[k][0] * s[2]
            + DCT8_ODD[k][1] * s[6]
            + DCT8_ODD[k][2] * s[10]
            + DCT8_ODD[k][3] * s[14];
    }

    // Combine into 8 even values.
    let mut e = [0i32; 8];
    for k in 0..4 {
        e[k] = ee[k] + eo[k];
        e[7 - k] = ee[k] - eo[k];
    }

    // -- Odd part (8-point), operating on odd-indexed coefficients --
    let mut o = [0i32; 8];
    for k in 0..8 {
        o[k] = DCT16_ODD[k][0] * s[1]
            + DCT16_ODD[k][1] * s[3]
            + DCT16_ODD[k][2] * s[5]
            + DCT16_ODD[k][3] * s[7]
            + DCT16_ODD[k][4] * s[9]
            + DCT16_ODD[k][5] * s[11]
            + DCT16_ODD[k][6] * s[13]
            + DCT16_ODD[k][7] * s[15];
    }

    // Butterfly combination.
    for k in 0..8 {
        dst[offset + k * stride] = clip_i16((e[k] + o[k] + add) >> shift);
        dst[offset + (15 - k) * stride] = clip_i16((e[k] - o[k] + add) >> shift);
    }
}

/// Partial butterfly 32-point inverse DCT.
///
/// Decomposes into a 16-point even part (recursing through 16/8/4 structure)
/// and a 16-point odd part (DCT32_ODD).
fn idct32(src: &[i32], dst: &mut [i32], offset: usize, stride: usize, shift: u32) {
    let add = 1i32 << (shift - 1);

    // Load all 32 inputs.
    let mut s = [0i32; 32];
    for i in 0..32 {
        s[i] = src[offset + i * stride];
    }

    // -- Even part (16-point), on even-indexed coefficients s[0,2,4,...,30] --

    // Level 2: 8-point even part on s[0,4,8,12,16,20,24,28].
    // Level 3: 4-point even-even on s[0,8,16,24].
    let eeee0 = DCT4[0][0] * s[0] + DCT4[0][1] * s[16]; // 64*(s0 + s16)
    let eeee1 = DCT4[0][0] * s[0] - DCT4[0][1] * s[16]; // 64*(s0 - s16)
    let eeeo0 = DCT4[1][0] * s[8] + DCT4[1][1] * s[24];  // 83*s8 + 36*s24
    let eeeo1 = DCT4[1][1] * s[8] - DCT4[1][0] * s[24];  // 36*s8 - 83*s24

    let eee = [
        eeee0 + eeeo0,
        eeee1 + eeeo1,
        eeee1 - eeeo1,
        eeee0 - eeeo0,
    ];

    // Level 3: 4-point even-odd on s[4,12,20,28].
    let mut eeo = [0i32; 4];
    for k in 0..4 {
        eeo[k] = DCT8_ODD[k][0] * s[4]
            + DCT8_ODD[k][1] * s[12]
            + DCT8_ODD[k][2] * s[20]
            + DCT8_ODD[k][3] * s[28];
    }

    // Combine into 8 even-even values.
    let mut ee = [0i32; 8];
    for k in 0..4 {
        ee[k] = eee[k] + eeo[k];
        ee[7 - k] = eee[k] - eeo[k];
    }

    // Level 2: 8-point odd part on s[2,6,10,14,18,22,26,30].
    let mut eo = [0i32; 8];
    for k in 0..8 {
        eo[k] = DCT16_ODD[k][0] * s[2]
            + DCT16_ODD[k][1] * s[6]
            + DCT16_ODD[k][2] * s[10]
            + DCT16_ODD[k][3] * s[14]
            + DCT16_ODD[k][4] * s[18]
            + DCT16_ODD[k][5] * s[22]
            + DCT16_ODD[k][6] * s[26]
            + DCT16_ODD[k][7] * s[30];
    }

    // Combine into 16 even values.
    let mut e = [0i32; 16];
    for k in 0..8 {
        e[k] = ee[k] + eo[k];
        e[15 - k] = ee[k] - eo[k];
    }

    // -- Odd part (16-point), on odd-indexed coefficients s[1,3,5,...,31] --
    let mut o = [0i32; 16];
    for k in 0..16 {
        o[k] = DCT32_ODD[k][0] * s[1]
            + DCT32_ODD[k][1] * s[3]
            + DCT32_ODD[k][2] * s[5]
            + DCT32_ODD[k][3] * s[7]
            + DCT32_ODD[k][4] * s[9]
            + DCT32_ODD[k][5] * s[11]
            + DCT32_ODD[k][6] * s[13]
            + DCT32_ODD[k][7] * s[15]
            + DCT32_ODD[k][8] * s[17]
            + DCT32_ODD[k][9] * s[19]
            + DCT32_ODD[k][10] * s[21]
            + DCT32_ODD[k][11] * s[23]
            + DCT32_ODD[k][12] * s[25]
            + DCT32_ODD[k][13] * s[27]
            + DCT32_ODD[k][14] * s[29]
            + DCT32_ODD[k][15] * s[31];
    }

    // Butterfly combination.
    for k in 0..16 {
        dst[offset + k * stride] = clip_i16((e[k] + o[k] + add) >> shift);
        dst[offset + (31 - k) * stride] = clip_i16((e[k] - o[k] + add) >> shift);
    }
}

// -- 2-D Inverse Transform --

/// Perform a 2-D inverse transform on a square block of coefficients.
///
/// The transform operates in two passes:
/// 1. Column-wise 1-D IDCT/IDST with shift = 7
/// 2. Row-wise 1-D IDCT/IDST with shift = 20 - bit_depth
///
/// After the transform, residual samples are clipped to the valid range for
/// the given bit depth.
///
/// # Parameters
/// - `coeffs`: flattened row-major NxN block (N = `size`). Modified in-place
///   from dequantized transform coefficients to spatial-domain residual samples.
/// - `size`: transform block size (4, 8, 16, or 32)
/// - `is_dst`: when true and size == 4, use the DST-VII kernel instead of DCT-II.
///   Ignored for sizes other than 4.
/// - `bit_depth`: sample bit depth (typically 8 or 10)
pub fn inverse_transform(coeffs: &mut [i32], size: u32, is_dst: bool, bit_depth: u8) {
    let n = size as usize;
    debug_assert!(
        matches!(n, 4 | 8 | 16 | 32),
        "HEVC transform size must be 4, 8, 16, or 32"
    );
    debug_assert!(
        coeffs.len() >= n * n,
        "coefficient buffer too small for {}x{} transform",
        n,
        n
    );

    // Column transform shift is always 7 (Section 8.6.4.2).
    let col_shift: u32 = 7;
    // Row transform shift depends on bit depth (Section 8.6.4.2).
    let row_shift: u32 = 20 - bit_depth as u32;

    // Scratch buffer for the intermediate (column-transformed) result.
    let mut tmp = vec![0i32; n * n];

    // Pass 1: column-wise transform. Each column has stride = n in row-major layout.
    // We read from `coeffs` (which stores data row-major, so column j starts at
    // index j and successive rows are at j, j+n, j+2n, ...) and write to `tmp`.
    match n {
        4 => {
            let kernel_col = if is_dst { idst4 } else { idct4 };
            for col in 0..4 {
                kernel_col(coeffs, &mut tmp, col, n, col_shift);
            }
        }
        8 => {
            for col in 0..8 {
                idct8(coeffs, &mut tmp, col, n, col_shift);
            }
        }
        16 => {
            for col in 0..16 {
                idct16(coeffs, &mut tmp, col, n, col_shift);
            }
        }
        32 => {
            for col in 0..32 {
                idct32(coeffs, &mut tmp, col, n, col_shift);
            }
        }
        _ => unreachable!(),
    }

    // Pass 2: row-wise transform. Each row has stride = 1.
    // We read from `tmp` and write back into `coeffs`.
    match n {
        4 => {
            let kernel_row = if is_dst { idst4 } else { idct4 };
            for row in 0..4 {
                kernel_row(&tmp, coeffs, row * n, 1, row_shift);
            }
        }
        8 => {
            for row in 0..8 {
                idct8(&tmp, coeffs, row * n, 1, row_shift);
            }
        }
        16 => {
            for row in 0..16 {
                idct16(&tmp, coeffs, row * n, 1, row_shift);
            }
        }
        32 => {
            for row in 0..32 {
                idct32(&tmp, coeffs, row * n, 1, row_shift);
            }
        }
        _ => unreachable!(),
    }

    // Clip residuals to valid range for the bit depth.
    for c in coeffs[..n * n].iter_mut() {
        *c = clip_residual(*c, bit_depth);
    }
}

/// Apply transform skip: bypass the inverse transform, applying only
/// the shift required to bring dequantized coefficients into the
/// residual sample domain.
///
/// When `transform_skip_flag` is set in the bitstream, the encoder
/// signals raw residual values (after quantization) rather than
/// frequency-domain coefficients. The decoder must still apply a
/// normalization shift.
///
/// # Parameters
/// - `coeffs`: dequantized coefficient levels, modified in-place to residuals
/// - `size`: block size (4, 8, 16, or 32)
/// - `bit_depth`: sample bit depth
#[cfg(test)]
pub fn transform_skip(coeffs: &mut [i32], size: u32, bit_depth: u8) {
    let n = size as usize;

    // Per spec Section 8.6.4.1, the transform skip shift is:
    //   tsShift = 5 + Log2(nTbS)
    // for the case where bdShift during dequant already accounted for bit_depth.
    //
    // The net shift for transform skip mode compensates for the two transform
    // shifts that would normally be applied (col_shift=7, row_shift=20-bd).
    // Since we skip both, we apply a single combined shift.
    let log2_size = match size {
        4 => 2u32,
        8 => 3,
        16 => 4,
        32 => 5,
        _ => panic!("invalid transform size"),
    };

    // tsShift = max(0, bit_depth + log2_size - 5)
    // This value comes from the dequant/transform normalization that would
    // otherwise happen across the two IDCT passes.
    let ts_shift = (bit_depth as i32 + log2_size as i32 - 5).max(0) as u32;

    if ts_shift > 0 {
        let add = 1i32 << (ts_shift - 1);
        for c in coeffs[..n * n].iter_mut() {
            *c = clip_residual((*c + add) >> ts_shift, bit_depth);
        }
    } else {
        for c in coeffs[..n * n].iter_mut() {
            *c = clip_residual(*c, bit_depth);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Dequantize tests --

    #[test]
    fn dequantize_zero_coefficients_stay_zero() {
        let mut coeffs = vec![0i32; 16];
        dequantize(&mut coeffs, 26, 8, 2);
        assert!(coeffs.iter().all(|&c| c == 0));
    }

    #[test]
    fn dequantize_single_dc_coefficient() {
        // qp=0: scale=40, qp_per=0, bd_shift = 8+2-5 = 5
        // d = (1 * 16 * 40) << 0 = 640; (640 + 16) >> 5 = 20
        let mut coeffs = vec![0i32; 16];
        coeffs[0] = 1;
        dequantize(&mut coeffs, 0, 8, 2);
        assert_eq!(coeffs[0], 20);
    }

    #[test]
    fn dequantize_respects_qp_scaling() {
        // Higher qp should produce larger dequantized values for the same level.
        let mut low = vec![0i32; 16];
        let mut high = vec![0i32; 16];
        low[0] = 5;
        high[0] = 5;

        dequantize(&mut low, 10, 8, 2);
        dequantize(&mut high, 30, 8, 2);

        assert!(
            high[0].abs() > low[0].abs(),
            "higher qp should produce larger magnitude: low={}, high={}",
            low[0],
            high[0]
        );
    }

    #[test]
    fn dequantize_clips_to_16bit_range() {
        let mut coeffs = vec![0i32; 16];
        coeffs[0] = 30000;
        dequantize(&mut coeffs, 40, 8, 2);
        assert!(coeffs[0] <= 32767);
        assert!(coeffs[0] >= -32768);
    }

    #[test]
    fn dequantize_negative_coefficient() {
        let mut coeffs = vec![0i32; 16];
        coeffs[0] = -1;
        dequantize(&mut coeffs, 0, 8, 2);
        assert_eq!(coeffs[0], -20);
    }

    #[test]
    fn dequantize_8x8_block() {
        let mut coeffs = vec![0i32; 64];
        coeffs[0] = 10;
        coeffs[1] = -5;
        dequantize(&mut coeffs, 22, 8, 3);
        // Just verify it runs and produces nonzero values.
        assert_ne!(coeffs[0], 0);
        assert_ne!(coeffs[1], 0);
        // Negative input should produce negative output.
        assert!(coeffs[1] < 0);
    }

    // -- Transform tests --

    #[test]
    fn idct4_dc_only() {
        // A DC-only 4x4 block: only coefficient [0,0] is nonzero.
        // After IDCT, all samples should be equal (flat block).
        let mut coeffs = vec![0i32; 16];
        coeffs[0] = 1024;
        inverse_transform(&mut coeffs, 4, false, 8);

        let first = coeffs[0];
        assert_ne!(first, 0, "DC-only block should produce nonzero residuals");
        for i in 1..16 {
            assert_eq!(
                coeffs[i], first,
                "DC-only block should be flat, but [0]={} != [{}]={}",
                first, i, coeffs[i]
            );
        }
    }

    #[test]
    fn idst4_dc_only() {
        // DST with DC-only input won't be perfectly flat (DST basis vectors
        // aren't constant), but should still produce valid output.
        let mut coeffs = vec![0i32; 16];
        coeffs[0] = 1024;
        inverse_transform(&mut coeffs, 4, true, 8);

        // Verify output is within residual range for 8-bit.
        for &c in &coeffs[..16] {
            assert!(c >= -128 && c <= 127, "residual {} out of 8-bit range", c);
        }
    }

    #[test]
    fn idct8_dc_only() {
        let mut coeffs = vec![0i32; 64];
        coeffs[0] = 2048;
        inverse_transform(&mut coeffs, 8, false, 8);

        let first = coeffs[0];
        assert_ne!(first, 0);
        for i in 1..64 {
            assert_eq!(coeffs[i], first, "8x8 DC-only not flat at index {}", i);
        }
    }

    #[test]
    fn idct16_dc_only() {
        let mut coeffs = vec![0i32; 256];
        coeffs[0] = 4096;
        inverse_transform(&mut coeffs, 16, false, 8);

        let first = coeffs[0];
        assert_ne!(first, 0);
        for i in 1..256 {
            assert_eq!(coeffs[i], first, "16x16 DC-only not flat at index {}", i);
        }
    }

    #[test]
    fn idct32_dc_only() {
        let mut coeffs = vec![0i32; 1024];
        coeffs[0] = 8192;
        inverse_transform(&mut coeffs, 32, false, 8);

        let first = coeffs[0];
        assert_ne!(first, 0);
        for i in 1..1024 {
            assert_eq!(coeffs[i], first, "32x32 DC-only not flat at index {}", i);
        }
    }

    #[test]
    fn idct4_all_zeros_stays_zero() {
        let mut coeffs = vec![0i32; 16];
        inverse_transform(&mut coeffs, 4, false, 8);
        assert!(coeffs.iter().all(|&c| c == 0));
    }

    #[test]
    fn idct8_all_zeros_stays_zero() {
        let mut coeffs = vec![0i32; 64];
        inverse_transform(&mut coeffs, 8, false, 8);
        assert!(coeffs.iter().all(|&c| c == 0));
    }

    #[test]
    fn inverse_transform_output_clipped_to_bit_depth() {
        // Large input values should still produce output within the residual range.
        let mut coeffs = vec![32000i32; 16];
        inverse_transform(&mut coeffs, 4, false, 8);
        for &c in &coeffs[..16] {
            assert!(c >= -128 && c <= 127, "residual {} out of 8-bit range", c);
        }
    }

    #[test]
    fn inverse_transform_10bit_range() {
        let mut coeffs = vec![0i32; 64];
        coeffs[0] = 2048;
        inverse_transform(&mut coeffs, 8, false, 10);
        for &c in &coeffs[..64] {
            assert!(
                c >= -512 && c <= 511,
                "residual {} out of 10-bit range",
                c
            );
        }
    }

    #[test]
    fn transform_skip_preserves_zeros() {
        let mut coeffs = vec![0i32; 16];
        transform_skip(&mut coeffs, 4, 8);
        assert!(coeffs.iter().all(|&c| c == 0));
    }

    #[test]
    fn transform_skip_applies_shift() {
        let mut coeffs = vec![0i32; 16];
        coeffs[0] = 256;
        transform_skip(&mut coeffs, 4, 8);
        // ts_shift = 8 + 2 - 5 = 5; (256 + 16) >> 5 = 8
        assert_eq!(coeffs[0], 8);
    }

    #[test]
    fn transform_skip_clips_residuals() {
        let mut coeffs = vec![32000i32; 16];
        transform_skip(&mut coeffs, 4, 8);
        for &c in &coeffs[..16] {
            assert!(c >= -128 && c <= 127, "residual {} out of 8-bit range", c);
        }
    }

    #[test]
    fn dequantize_then_transform_roundtrip_smoke() {
        // Smoke test: dequantize a sparse block, then transform it.
        // Verify no panics and output is within valid range.
        let mut coeffs = vec![0i32; 64];
        coeffs[0] = 50;
        coeffs[1] = -20;
        coeffs[8] = 10;

        dequantize(&mut coeffs, 26, 8, 3);
        inverse_transform(&mut coeffs, 8, false, 8);

        for &c in &coeffs[..64] {
            assert!(
                c >= -128 && c <= 127,
                "residual {} out of 8-bit range",
                c
            );
        }
    }

    // -- Butterfly orthogonality verification --
    // The 4x4 DCT matrix should satisfy: T * T^T = 4096 * I (since coefficients
    // are scaled by 64). Verify by forward-then-inverse on a known signal.

    #[test]
    fn idct4_known_pattern() {
        // Set up a pattern with nonzero AC coefficients to verify the butterfly
        // produces distinct spatial values (not all the same).
        let mut coeffs = vec![0i32; 16];
        coeffs[0] = 512;  // DC
        coeffs[1] = 256;  // first AC column 0
        coeffs[4] = 128;  // first AC row 1
        inverse_transform(&mut coeffs, 4, false, 8);

        // With AC coefficients, not all samples should be identical.
        let all_same = coeffs[..16].iter().all(|&c| c == coeffs[0]);
        assert!(!all_same, "AC coefficients should produce non-flat output");
    }

    #[test]
    fn idct4_symmetry() {
        // A symmetric frequency pattern should produce a symmetric spatial block.
        // coeffs[0] = DC, coeffs[2] (row 2, col 0) = AC.
        // The DCT basis vector 2 is [64, -64, -64, 64], which is symmetric about center.
        let mut coeffs = vec![0i32; 16];
        coeffs[0] = 1024;
        coeffs[8] = 512; // Row 2, col 0: even-symmetric basis
        inverse_transform(&mut coeffs, 4, false, 8);

        // Rows 0 and 3 should be equal (even symmetry of basis vector 2).
        for col in 0..4 {
            assert_eq!(
                coeffs[col],
                coeffs[3 * 4 + col],
                "rows 0 and 3 should match for even-symmetric AC"
            );
        }
        // Rows 1 and 2 should be equal.
        for col in 0..4 {
            assert_eq!(
                coeffs[4 + col],
                coeffs[2 * 4 + col],
                "rows 1 and 2 should match for even-symmetric AC"
            );
        }
    }

    #[test]
    fn idct32_matches_direct_matrix_multiply() {
        // Full 32x32 DCT matrix (H.265 Tables 8-3 through 8-6).
        const T: [[i32; 32]; 32] = [
            [ 64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,
              64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64,  64],
            [ 90,  90,  88,  85,  82,  78,  73,  67,  61,  54,  46,  38,  31,  22,  13,   4,
              -4, -13, -22, -31, -38, -46, -54, -61, -67, -73, -78, -82, -85, -88, -90, -90],
            [ 90,  87,  80,  70,  57,  43,  25,   9,  -9, -25, -43, -57, -70, -80, -87, -90,
             -90, -87, -80, -70, -57, -43, -25,  -9,   9,  25,  43,  57,  70,  80,  87,  90],
            [ 90,  82,  67,  46,  22,  -4, -31, -54, -73, -85, -90, -88, -78, -61, -38, -13,
              13,  38,  61,  78,  88,  90,  85,  73,  54,  31,   4, -22, -46, -67, -82, -90],
            [ 89,  75,  50,  18, -18, -50, -75, -89, -89, -75, -50, -18,  18,  50,  75,  89,
              89,  75,  50,  18, -18, -50, -75, -89, -89, -75, -50, -18,  18,  50,  75,  89],
            [ 88,  67,  31, -13, -54, -82, -90, -78, -46,  -4,  38,  73,  90,  85,  61,  22,
             -22, -61, -85, -90, -73, -38,   4,  46,  78,  90,  82,  54,  13, -31, -67, -88],
            [ 87,  57,   9, -43, -80, -90, -70, -25,  25,  70,  90,  80,  43,  -9, -57, -87,
             -87, -57,  -9,  43,  80,  90,  70,  25, -25, -70, -90, -80, -43,   9,  57,  87],
            [ 85,  46, -13, -67, -90, -73, -22,  38,  82,  88,  54,  -4, -61, -90, -78, -31,
              31,  78,  90,  61,   4, -54, -88, -82, -38,  22,  73,  90,  67,  13, -46, -85],
            [ 83,  36, -36, -83, -83, -36,  36,  83,  83,  36, -36, -83, -83, -36,  36,  83,
              83,  36, -36, -83, -83, -36,  36,  83,  83,  36, -36, -83, -83, -36,  36,  83],
            [ 82,  22, -54, -90, -61,  13,  78,  85,  31, -46, -90, -67,   4,  73,  88,  38,
             -38, -88, -73,  -4,  67,  90,  46, -31, -85, -78, -13,  61,  90,  54, -22, -82],
            [ 80,   9, -70, -87, -25,  57,  90,  43, -43, -90, -57,  25,  87,  70,  -9, -80,
             -80,  -9,  70,  87,  25, -57, -90, -43,  43,  90,  57, -25, -87, -70,   9,  80],
            [ 78,  -4, -82, -73,  13,  85,  67, -22, -88, -61,  31,  90,  54, -38, -90, -46,
              46,  90,  38, -54, -90, -31,  61,  88,  22, -67, -85, -13,  73,  82,   4, -78],
            [ 75, -18, -89, -50,  50,  89,  18, -75, -75,  18,  89,  50, -50, -89, -18,  75,
              75, -18, -89, -50,  50,  89,  18, -75, -75,  18,  89,  50, -50, -89, -18,  75],
            [ 73, -31, -90, -22,  78,  67, -38, -90, -13,  82,  61, -46, -88,  -4,  85,  54,
             -54, -85,   4,  88,  46, -61, -82,  13,  90,  38, -67, -78,  22,  90,  31, -73],
            [ 70, -43, -87,   9,  90,  25, -80, -57,  57,  80, -25, -90,  -9,  87,  43, -70,
             -70,  43,  87,  -9, -90, -25,  80,  57, -57, -80,  25,  90,   9, -87, -43,  70],
            [ 67, -54, -78,  38,  85, -22, -90,   4,  90,  13, -88, -31,  82,  46, -73, -61,
              61,  73, -46, -82,  31,  88, -13, -90,  -4,  90,  22, -85, -38,  78,  54, -67],
            [ 64, -64, -64,  64,  64, -64, -64,  64,  64, -64, -64,  64,  64, -64, -64,  64,
              64, -64, -64,  64,  64, -64, -64,  64,  64, -64, -64,  64,  64, -64, -64,  64],
            [ 61, -73, -46,  82,  31, -88, -13,  90,  -4, -90,  22,  85, -38, -78,  54,  67,
             -67, -54,  78,  38, -85, -22,  90,   4, -90,  13,  88, -31, -82,  46,  73, -61],
            [ 57, -80, -25,  90,  -9, -87,  43,  70, -70, -43,  87,   9, -90,  25,  80, -57,
             -57,  80,  25, -90,   9,  87, -43, -70,  70,  43, -87,  -9,  90, -25, -80,  57],
            [ 54, -85,  -4,  88, -46, -61,  82,  13, -90,  38,  67, -78, -22,  90, -31, -73,
              73,  31, -90,  22,  78, -67, -38,  90, -13, -82,  61,  46, -88,   4,  85, -54],
            [ 50, -89,  18,  75, -75, -18,  89, -50, -50,  89, -18, -75,  75,  18, -89,  50,
              50, -89,  18,  75, -75, -18,  89, -50, -50,  89, -18, -75,  75,  18, -89,  50],
            [ 46, -90,  38,  54, -90,  31,  61, -88,  22,  67, -85,  13,  73, -82,   4,  78,
             -78,  -4,  82, -73, -13,  85, -67, -22,  88, -61, -31,  90, -54, -38,  90, -46],
            [ 43, -90,  57,  25, -87,  70,   9, -80,  80,  -9, -70,  87, -25, -57,  90, -43,
             -43,  90, -57, -25,  87, -70,  -9,  80, -80,   9,  70, -87,  25,  57, -90,  43],
            [ 38, -88,  73,  -4, -67,  90, -46, -31,  85, -78,  13,  61, -90,  54,  22, -82,
              82, -22, -54,  90, -61, -13,  78, -85,  31,  46, -90,  67,   4, -73,  88, -38],
            [ 36, -83,  83, -36, -36,  83, -83,  36,  36, -83,  83, -36, -36,  83, -83,  36,
              36, -83,  83, -36, -36,  83, -83,  36,  36, -83,  83, -36, -36,  83, -83,  36],
            [ 31, -78,  90, -61,   4,  54, -88,  82, -38, -22,  73, -90,  67, -13, -46,  85,
             -85,  46,  13, -67,  90, -73,  22,  38, -82,  88, -54,  -4,  61, -90,  78, -31],
            [ 25, -70,  90, -80,  43,   9, -57,  87, -87,  57,  -9, -43,  80, -90,  70, -25,
             -25,  70, -90,  80, -43,  -9,  57, -87,  87, -57,   9,  43, -80,  90, -70,  25],
            [ 22, -61,  85, -90,  73, -38,  -4,  46, -78,  90, -82,  54, -13, -31,  67, -88,
              88, -67,  31,  13, -54,  82, -90,  78, -46,   4,  38, -73,  90, -85,  61, -22],
            [ 18, -50,  75, -89,  89, -75,  50, -18, -18,  50, -75,  89, -89,  75, -50,  18,
              18, -50,  75, -89,  89, -75,  50, -18, -18,  50, -75,  89, -89,  75, -50,  18],
            [ 13, -38,  61, -78,  88, -90,  85, -73,  54, -31,   4,  22, -46,  67, -82,  90,
             -90,  82, -67,  46, -22,  -4,  31, -54,  73, -85,  90, -88,  78, -61,  38, -13],
            [  9, -25,  43, -57,  70, -80,  87, -90,  90, -87,  80, -70,  57, -43,  25,  -9,
              -9,  25, -43,  57, -70,  80, -87,  90, -90,  87, -80,  70, -57,  43, -25,   9],
            [  4, -13,  22, -31,  38, -46,  54, -61,  67, -73,  78, -82,  85, -88,  90, -90,
              90, -90,  88, -85,  82, -78,  73, -67,  61, -54,  46, -38,  31, -22,  13,  -4],
        ];

        // Sparse 32x32 block from a real HEIC decode (sideways2.heic CTU near pixel 288,0).
        let nz: [(usize, i32); 40] = [
            (0,-45),(1,81),(2,-18),(3,9),(7,-9),(9,-9),(32,-9),(33,-27),(34,18),(37,9),
            (38,-9),(65,-9),(66,9),(67,9),(69,-18),(73,-9),(97,-9),(98,-9),(102,9),
            (104,-9),(129,-9),(130,-9),(131,9),(161,9),(162,9),(163,9),(165,-9),(168,-9),
            (170,-9),(193,9),(195,9),(198,9),(228,9),(233,9),(262,9),(263,9),(264,9),
            (267,-9),(298,9),(327,9),
        ];
        let mut coeffs_butterfly = vec![0i32; 1024];
        let mut coeffs_ref = vec![0i32; 1024];
        for &(idx, val) in &nz {
            coeffs_butterfly[idx] = val;
            coeffs_ref[idx] = val;
        }

        // Reference: direct matrix multiply (column pass then row pass).
        let col_shift = 7u32;
        let row_shift = 12u32;
        let mut ref_tmp = vec![0i32; 1024];
        for col in 0..32usize {
            let add = 1i32 << (col_shift - 1);
            for out_row in 0..32usize {
                let mut sum = 0i64;
                for k in 0..32usize {
                    sum += T[k][out_row] as i64 * coeffs_ref[k * 32 + col] as i64;
                }
                ref_tmp[out_row * 32 + col] = clip_i16((sum as i32 + add) >> col_shift);
            }
        }
        let mut reference = vec![0i32; 1024];
        for row in 0..32usize {
            let add = 1i32 << (row_shift - 1);
            for out_col in 0..32usize {
                let mut sum = 0i64;
                for k in 0..32usize {
                    sum += T[k][out_col] as i64 * ref_tmp[row * 32 + k] as i64;
                }
                reference[row * 32 + out_col] =
                    clip_residual(clip_i16((sum as i32 + add) >> row_shift), 8);
            }
        }

        // Our butterfly implementation.
        inverse_transform(&mut coeffs_butterfly, 32, false, 8);

        for i in 0..1024 {
            assert_eq!(
                coeffs_butterfly[i], reference[i],
                "mismatch at ({}, {}): butterfly={} reference={}",
                i % 32, i / 32, coeffs_butterfly[i], reference[i]
            );
        }

        // Verify that output[0] is 0 (not 1) for these specific coefficients.
        assert_eq!(coeffs_butterfly[0], 0);
    }
}
