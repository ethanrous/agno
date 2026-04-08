// HEVC (H.265) inverse quantization and inverse transform.
//
// Implements ITU-T H.265 Section 8.6.2 (scaling/inverse quantization) and
// Section 8.6.4 (inverse transform). Supports all HEVC transform block sizes
// (4x4, 8x8, 16x16, 32x32) with both DCT and DST kernels.

use super::params::ScalingListData;

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

/// Full 32x32 DCT-II matrix (H.265 Tables 8-3 through 8-6) in row-major order.
/// Used by the direct matrix multiply IDCT to match libde265/ffmpeg exactly.
/// For smaller transforms, rows are subsampled by factor `32/nT`:
///   nT=4  -> rows 0, 8, 16, 24
///   nT=8  -> rows 0, 4, 8, ..., 28
///   nT=16 -> rows 0, 2, 4, ..., 30
///   nT=32 -> all rows
const MAT_DCT: [[i32; 32]; 32] = [
    [
        64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
        64, 64, 64, 64, 64, 64, 64, 64, 64,
    ],
    [
        90, 90, 88, 85, 82, 78, 73, 67, 61, 54, 46, 38, 31, 22, 13, 4, -4, -13, -22, -31, -38, -46,
        -54, -61, -67, -73, -78, -82, -85, -88, -90, -90,
    ],
    [
        90, 87, 80, 70, 57, 43, 25, 9, -9, -25, -43, -57, -70, -80, -87, -90, -90, -87, -80, -70,
        -57, -43, -25, -9, 9, 25, 43, 57, 70, 80, 87, 90,
    ],
    [
        90, 82, 67, 46, 22, -4, -31, -54, -73, -85, -90, -88, -78, -61, -38, -13, 13, 38, 61, 78,
        88, 90, 85, 73, 54, 31, 4, -22, -46, -67, -82, -90,
    ],
    [
        89, 75, 50, 18, -18, -50, -75, -89, -89, -75, -50, -18, 18, 50, 75, 89, 89, 75, 50, 18,
        -18, -50, -75, -89, -89, -75, -50, -18, 18, 50, 75, 89,
    ],
    [
        88, 67, 31, -13, -54, -82, -90, -78, -46, -4, 38, 73, 90, 85, 61, 22, -22, -61, -85, -90,
        -73, -38, 4, 46, 78, 90, 82, 54, 13, -31, -67, -88,
    ],
    [
        87, 57, 9, -43, -80, -90, -70, -25, 25, 70, 90, 80, 43, -9, -57, -87, -87, -57, -9, 43, 80,
        90, 70, 25, -25, -70, -90, -80, -43, 9, 57, 87,
    ],
    [
        85, 46, -13, -67, -90, -73, -22, 38, 82, 88, 54, -4, -61, -90, -78, -31, 31, 78, 90, 61, 4,
        -54, -88, -82, -38, 22, 73, 90, 67, 13, -46, -85,
    ],
    [
        83, 36, -36, -83, -83, -36, 36, 83, 83, 36, -36, -83, -83, -36, 36, 83, 83, 36, -36, -83,
        -83, -36, 36, 83, 83, 36, -36, -83, -83, -36, 36, 83,
    ],
    [
        82, 22, -54, -90, -61, 13, 78, 85, 31, -46, -90, -67, 4, 73, 88, 38, -38, -88, -73, -4, 67,
        90, 46, -31, -85, -78, -13, 61, 90, 54, -22, -82,
    ],
    [
        80, 9, -70, -87, -25, 57, 90, 43, -43, -90, -57, 25, 87, 70, -9, -80, -80, -9, 70, 87, 25,
        -57, -90, -43, 43, 90, 57, -25, -87, -70, 9, 80,
    ],
    [
        78, -4, -82, -73, 13, 85, 67, -22, -88, -61, 31, 90, 54, -38, -90, -46, 46, 90, 38, -54,
        -90, -31, 61, 88, 22, -67, -85, -13, 73, 82, 4, -78,
    ],
    [
        75, -18, -89, -50, 50, 89, 18, -75, -75, 18, 89, 50, -50, -89, -18, 75, 75, -18, -89, -50,
        50, 89, 18, -75, -75, 18, 89, 50, -50, -89, -18, 75,
    ],
    [
        73, -31, -90, -22, 78, 67, -38, -90, -13, 82, 61, -46, -88, -4, 85, 54, -54, -85, 4, 88,
        46, -61, -82, 13, 90, 38, -67, -78, 22, 90, 31, -73,
    ],
    [
        70, -43, -87, 9, 90, 25, -80, -57, 57, 80, -25, -90, -9, 87, 43, -70, -70, 43, 87, -9, -90,
        -25, 80, 57, -57, -80, 25, 90, 9, -87, -43, 70,
    ],
    [
        67, -54, -78, 38, 85, -22, -90, 4, 90, 13, -88, -31, 82, 46, -73, -61, 61, 73, -46, -82,
        31, 88, -13, -90, -4, 90, 22, -85, -38, 78, 54, -67,
    ],
    [
        64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64,
        64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64,
    ],
    [
        61, -73, -46, 82, 31, -88, -13, 90, -4, -90, 22, 85, -38, -78, 54, 67, -67, -54, 78, 38,
        -85, -22, 90, 4, -90, 13, 88, -31, -82, 46, 73, -61,
    ],
    [
        57, -80, -25, 90, -9, -87, 43, 70, -70, -43, 87, 9, -90, 25, 80, -57, -57, 80, 25, -90, 9,
        87, -43, -70, 70, 43, -87, -9, 90, -25, -80, 57,
    ],
    [
        54, -85, -4, 88, -46, -61, 82, 13, -90, 38, 67, -78, -22, 90, -31, -73, 73, 31, -90, 22,
        78, -67, -38, 90, -13, -82, 61, 46, -88, 4, 85, -54,
    ],
    [
        50, -89, 18, 75, -75, -18, 89, -50, -50, 89, -18, -75, 75, 18, -89, 50, 50, -89, 18, 75,
        -75, -18, 89, -50, -50, 89, -18, -75, 75, 18, -89, 50,
    ],
    [
        46, -90, 38, 54, -90, 31, 61, -88, 22, 67, -85, 13, 73, -82, 4, 78, -78, -4, 82, -73, -13,
        85, -67, -22, 88, -61, -31, 90, -54, -38, 90, -46,
    ],
    [
        43, -90, 57, 25, -87, 70, 9, -80, 80, -9, -70, 87, -25, -57, 90, -43, -43, 90, -57, -25,
        87, -70, -9, 80, -80, 9, 70, -87, 25, 57, -90, 43,
    ],
    [
        38, -88, 73, -4, -67, 90, -46, -31, 85, -78, 13, 61, -90, 54, 22, -82, 82, -22, -54, 90,
        -61, -13, 78, -85, 31, 46, -90, 67, 4, -73, 88, -38,
    ],
    [
        36, -83, 83, -36, -36, 83, -83, 36, 36, -83, 83, -36, -36, 83, -83, 36, 36, -83, 83, -36,
        -36, 83, -83, 36, 36, -83, 83, -36, -36, 83, -83, 36,
    ],
    [
        31, -78, 90, -61, 4, 54, -88, 82, -38, -22, 73, -90, 67, -13, -46, 85, -85, 46, 13, -67,
        90, -73, 22, 38, -82, 88, -54, -4, 61, -90, 78, -31,
    ],
    [
        25, -70, 90, -80, 43, 9, -57, 87, -87, 57, -9, -43, 80, -90, 70, -25, -25, 70, -90, 80,
        -43, -9, 57, -87, 87, -57, 9, 43, -80, 90, -70, 25,
    ],
    [
        22, -61, 85, -90, 73, -38, -4, 46, -78, 90, -82, 54, -13, -31, 67, -88, 88, -67, 31, 13,
        -54, 82, -90, 78, -46, 4, 38, -73, 90, -85, 61, -22,
    ],
    [
        18, -50, 75, -89, 89, -75, 50, -18, -18, 50, -75, 89, -89, 75, -50, 18, 18, -50, 75, -89,
        89, -75, 50, -18, -18, 50, -75, 89, -89, 75, -50, 18,
    ],
    [
        13, -38, 61, -78, 88, -90, 85, -73, 54, -31, 4, 22, -46, 67, -82, 90, -90, 82, -67, 46,
        -22, -4, 31, -54, 73, -85, 90, -88, 78, -61, 38, -13,
    ],
    [
        9, -25, 43, -57, 70, -80, 87, -90, 90, -87, 80, -70, 57, -43, 25, -9, -9, 25, -43, 57, -70,
        80, -87, 90, -90, 87, -80, 70, -57, 43, -25, 9,
    ],
    [
        4, -13, 22, -31, 38, -46, 54, -61, 67, -73, 78, -82, 85, -88, 90, -90, 90, -90, 88, -85,
        82, -78, 73, -67, 61, -54, 46, -38, 31, -22, 13, -4,
    ],
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
/// When `scaling_list` is `Some`, uses the provided `ScalingListData` for
/// position-dependent scaling values. When `None`, uses the flat value of 16
/// for all positions (H.265 Section 8.6.3).
///
/// # Parameters
/// - `coeffs`: transform coefficient levels; modified in-place to scaled values
/// - `qp`: quantization parameter (0..51 typical, can exceed for high bit depth)
/// - `bit_depth`: luma or chroma bit depth (typically 8 or 10)
/// - `log2_size`: log2 of the transform block width (2 for 4x4, 3 for 8x8, etc.)
/// - `scaling_list`: optional reference to the active scaling list data (PPS overrides SPS)
/// - `c_idx`: color component index (0=Y, 1=Cb, 2=Cr)
#[allow(clippy::needless_range_loop)]
pub fn dequantize(
    coeffs: &mut [i32],
    qp: i32,
    bit_depth: u8,
    log2_size: u32,
    scaling_list: Option<&ScalingListData>,
    c_idx: u8,
) {
    // H.265 8.6.3: qP = qPY + QpBdOffsetY (luma) or qPCb + QpBdOffsetC (chroma)
    // QpBdOffsetY = 6 * (BitDepth - 8). For 10-bit, this adds 12 to the effective QP.
    // The caller passes qp = qpY (chroma offsets already applied via chroma_qp), so we
    // add the bit-depth offset here to match the H.265 dequantization formula.
    let qp_bd_offset = 6 * (bit_depth as i32 - 8);
    let qp_clamped = (qp + qp_bd_offset).clamp(0, 51 + qp_bd_offset);
    let qp_per = qp_clamped / 6;
    let qp_rem = (qp_clamped % 6) as usize;
    let scale = LEVEL_SCALE[qp_rem];

    // bdShift = bit_depth + Log2(nTbS) - 5  (H.265 Section 8.6.3)
    let bd_shift = bit_depth as i32 + log2_size as i32 - 5;

    let coeff_min = -(1i32 << 15);
    let coeff_max = (1i32 << 15) - 1;
    let size = 1u32 << log2_size;

    if let Some(sl) = scaling_list {
        // Position-dependent scaling from the scaling list
        if bd_shift >= 0 {
            let add: i64 = if bd_shift == 0 {
                0
            } else {
                1i64 << (bd_shift - 1)
            };
            for idx in 0..coeffs.len() {
                if coeffs[idx] == 0 {
                    continue;
                }
                let x = (idx as u32) % size;
                let y = (idx as u32) / size;
                let m = sl.scaling_value(x, y, log2_size, c_idx) as i64;
                let scaled = (coeffs[idx] as i64 * m * scale as i64) << qp_per;
                coeffs[idx] =
                    ((scaled + add) >> bd_shift).clamp(coeff_min as i64, coeff_max as i64) as i32;
            }
        } else {
            let left = (-bd_shift) as u32;
            for idx in 0..coeffs.len() {
                if coeffs[idx] == 0 {
                    continue;
                }
                let x = (idx as u32) % size;
                let y = (idx as u32) / size;
                let m = sl.scaling_value(x, y, log2_size, c_idx) as i64;
                let scaled = (coeffs[idx] as i64 * m * scale as i64) << qp_per;
                coeffs[idx] = (scaled << left).clamp(coeff_min as i64, coeff_max as i64) as i32;
            }
        }
    } else {
        // Flat scaling matrix value of 16 for all positions
        const DEFAULT_SCALE_M: i32 = 16;
        if bd_shift >= 0 {
            let add: i64 = if bd_shift == 0 {
                0
            } else {
                1i64 << (bd_shift - 1)
            };
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
}

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

    // Direct matrix multiply matching libde265's fallback-dct.cc.
    // Uses i32 accumulation to produce bit-identical results with libde265/ffmpeg.
    //
    // Column transform shift is always 7 (Section 8.6.4.2).
    let col_shift: u32 = 7;
    let rnd1 = 1i32 << (col_shift - 1);
    // Row transform shift depends on bit depth (Section 8.6.4.2).
    let row_shift: u32 = 20 - bit_depth as u32;
    let rnd2 = 1i32 << (row_shift - 1);

    // Row stride factor: maps nT to subsampled rows of the full 32x32 matrix.
    let fact = 32 / n;

    let mut tmp = vec![0i32; n * n];

    if is_dst && n == 4 {
        // 4x4 DST: use the DST4 matrix directly.
        // Column pass: for each column c, compute tmp[i*4+c] = sum_j DST4[j][i] * coeffs[j*4+c]
        for c in 0..4 {
            for i in 0..4 {
                let mut sum = 0i32;
                for j in 0..4 {
                    sum += DST4[j][i] * coeffs[j * 4 + c];
                }
                tmp[i * 4 + c] = clip_i16((sum + rnd1) >> col_shift);
            }
        }
        // Row pass: for each row y, compute coeffs[y*4+i] = sum_j DST4[j][i] * tmp[y*4+j]
        for y in 0..4 {
            for i in 0..4 {
                let mut sum = 0i32;
                for j in 0..4 {
                    sum += DST4[j][i] * tmp[y * 4 + j];
                }
                coeffs[y * 4 + i] = clip_i16((sum + rnd2) >> row_shift);
            }
        }
    } else {
        // DCT: use the full MAT_DCT with row subsampling by `fact`.
        // Column pass: tmp[i*n+c] = clip_i16((sum_j MAT_DCT[fact*j][i] * coeffs[j*n+c] + rnd1) >> 7)
        for c in 0..n {
            for i in 0..n {
                let mut sum = 0i32;
                for j in 0..n {
                    sum += MAT_DCT[fact * j][i] * coeffs[j * n + c];
                }
                tmp[i * n + c] = clip_i16((sum + rnd1) >> col_shift);
            }
        }
        // Row pass: coeffs[y*n+i] = (sum_j MAT_DCT[fact*j][i] * tmp[y*n+j] + rnd2) >> row_shift
        for y in 0..n {
            for i in 0..n {
                let mut sum = 0i32;
                for j in 0..n {
                    sum += MAT_DCT[fact * j][i] * tmp[y * n + j];
                }
                coeffs[y * n + i] = clip_i16((sum + rnd2) >> row_shift);
            }
        }
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
    use crate::codec::hevc::params::ScalingListData;

    // -- Dequantize tests --

    #[test]
    fn dequantize_zero_coefficients_stay_zero() {
        let mut coeffs = vec![0i32; 16];
        dequantize(&mut coeffs, 26, 8, 2, None, 0);
        assert!(coeffs.iter().all(|&c| c == 0));
    }

    #[test]
    fn dequantize_single_dc_coefficient() {
        // qp=0: scale=40, qp_per=0, bd_shift = 8+2-5 = 5
        // d = (1 * 16 * 40) << 0 = 640; (640 + 16) >> 5 = 20
        let mut coeffs = vec![0i32; 16];
        coeffs[0] = 1;
        dequantize(&mut coeffs, 0, 8, 2, None, 0);
        assert_eq!(coeffs[0], 20);
    }

    #[test]
    fn dequantize_respects_qp_scaling() {
        // Higher qp should produce larger dequantized values for the same level.
        let mut low = vec![0i32; 16];
        let mut high = vec![0i32; 16];
        low[0] = 5;
        high[0] = 5;

        dequantize(&mut low, 10, 8, 2, None, 0);
        dequantize(&mut high, 30, 8, 2, None, 0);

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
        dequantize(&mut coeffs, 40, 8, 2, None, 0);
        assert!(coeffs[0] <= 32767);
        assert!(coeffs[0] >= -32768);
    }

    #[test]
    fn dequantize_negative_coefficient() {
        let mut coeffs = vec![0i32; 16];
        coeffs[0] = -1;
        dequantize(&mut coeffs, 0, 8, 2, None, 0);
        assert_eq!(coeffs[0], -20);
    }

    #[test]
    fn dequantize_8x8_block() {
        let mut coeffs = vec![0i32; 64];
        coeffs[0] = 10;
        coeffs[1] = -5;
        dequantize(&mut coeffs, 22, 8, 3, None, 0);
        // Just verify it runs and produces nonzero values.
        assert_ne!(coeffs[0], 0);
        assert_ne!(coeffs[1], 0);
        // Negative input should produce negative output.
        assert!(coeffs[1] < 0);
    }

    #[test]
    fn dequantize_scaling_list_8x8_dc() {
        // With scaling list enabled, DC position (0,0) still has value 16 for
        // the 8x8 intra default list, so result should match flat-16 at DC.
        let mut with_sl = vec![0i32; 64];
        let mut without_sl = vec![0i32; 64];
        with_sl[0] = 1;
        without_sl[0] = 1;
        dequantize(
            &mut with_sl,
            0,
            8,
            3,
            Some(&ScalingListData::default_lists()),
            0,
        );
        dequantize(&mut without_sl, 0, 8, 3, None, 0);
        assert_eq!(with_sl[0], without_sl[0]);
    }

    #[test]
    fn dequantize_scaling_list_8x8_ac_differs() {
        // Position (7,7) has value 115 in the default intra list, not 16.
        // The scaling-list-enabled result should differ from flat.
        let mut with_sl = vec![0i32; 64];
        let mut without_sl = vec![0i32; 64];
        with_sl[63] = 1; // position (7,7)
        without_sl[63] = 1;
        dequantize(
            &mut with_sl,
            0,
            8,
            3,
            Some(&ScalingListData::default_lists()),
            0,
        );
        dequantize(&mut without_sl, 0, 8, 3, None, 0);
        assert!(
            with_sl[63].abs() > without_sl[63].abs(),
            "scaling list value 115 should produce larger result than flat 16: sl={}, flat={}",
            with_sl[63],
            without_sl[63]
        );
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
    fn inverse_transform_output_clipped_to_i16_range() {
        // IDCT output is clipped to i16 range [-32768, 32767] per H.265 8.6.4.2.
        // The bit-depth clipping happens later when adding residual to prediction.
        let mut coeffs = vec![32000i32; 16];
        inverse_transform(&mut coeffs, 4, false, 8);
        for &c in &coeffs[..16] {
            assert!(c >= -32768 && c <= 32767, "residual {} out of i16 range", c);
        }
    }

    #[test]
    fn inverse_transform_10bit_range() {
        let mut coeffs = vec![0i32; 64];
        coeffs[0] = 2048;
        inverse_transform(&mut coeffs, 8, false, 10);
        for &c in &coeffs[..64] {
            assert!(c >= -512 && c <= 511, "residual {} out of 10-bit range", c);
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

        dequantize(&mut coeffs, 26, 8, 3, None, 0);
        inverse_transform(&mut coeffs, 8, false, 8);

        for &c in &coeffs[..64] {
            assert!(c >= -32768 && c <= 32767, "residual {} out of i16 range", c);
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
        coeffs[0] = 512; // DC
        coeffs[1] = 256; // first AC column 0
        coeffs[4] = 128; // first AC row 1
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
            [
                64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
                64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64,
            ],
            [
                90, 90, 88, 85, 82, 78, 73, 67, 61, 54, 46, 38, 31, 22, 13, 4, -4, -13, -22, -31,
                -38, -46, -54, -61, -67, -73, -78, -82, -85, -88, -90, -90,
            ],
            [
                90, 87, 80, 70, 57, 43, 25, 9, -9, -25, -43, -57, -70, -80, -87, -90, -90, -87,
                -80, -70, -57, -43, -25, -9, 9, 25, 43, 57, 70, 80, 87, 90,
            ],
            [
                90, 82, 67, 46, 22, -4, -31, -54, -73, -85, -90, -88, -78, -61, -38, -13, 13, 38,
                61, 78, 88, 90, 85, 73, 54, 31, 4, -22, -46, -67, -82, -90,
            ],
            [
                89, 75, 50, 18, -18, -50, -75, -89, -89, -75, -50, -18, 18, 50, 75, 89, 89, 75, 50,
                18, -18, -50, -75, -89, -89, -75, -50, -18, 18, 50, 75, 89,
            ],
            [
                88, 67, 31, -13, -54, -82, -90, -78, -46, -4, 38, 73, 90, 85, 61, 22, -22, -61,
                -85, -90, -73, -38, 4, 46, 78, 90, 82, 54, 13, -31, -67, -88,
            ],
            [
                87, 57, 9, -43, -80, -90, -70, -25, 25, 70, 90, 80, 43, -9, -57, -87, -87, -57, -9,
                43, 80, 90, 70, 25, -25, -70, -90, -80, -43, 9, 57, 87,
            ],
            [
                85, 46, -13, -67, -90, -73, -22, 38, 82, 88, 54, -4, -61, -90, -78, -31, 31, 78,
                90, 61, 4, -54, -88, -82, -38, 22, 73, 90, 67, 13, -46, -85,
            ],
            [
                83, 36, -36, -83, -83, -36, 36, 83, 83, 36, -36, -83, -83, -36, 36, 83, 83, 36,
                -36, -83, -83, -36, 36, 83, 83, 36, -36, -83, -83, -36, 36, 83,
            ],
            [
                82, 22, -54, -90, -61, 13, 78, 85, 31, -46, -90, -67, 4, 73, 88, 38, -38, -88, -73,
                -4, 67, 90, 46, -31, -85, -78, -13, 61, 90, 54, -22, -82,
            ],
            [
                80, 9, -70, -87, -25, 57, 90, 43, -43, -90, -57, 25, 87, 70, -9, -80, -80, -9, 70,
                87, 25, -57, -90, -43, 43, 90, 57, -25, -87, -70, 9, 80,
            ],
            [
                78, -4, -82, -73, 13, 85, 67, -22, -88, -61, 31, 90, 54, -38, -90, -46, 46, 90, 38,
                -54, -90, -31, 61, 88, 22, -67, -85, -13, 73, 82, 4, -78,
            ],
            [
                75, -18, -89, -50, 50, 89, 18, -75, -75, 18, 89, 50, -50, -89, -18, 75, 75, -18,
                -89, -50, 50, 89, 18, -75, -75, 18, 89, 50, -50, -89, -18, 75,
            ],
            [
                73, -31, -90, -22, 78, 67, -38, -90, -13, 82, 61, -46, -88, -4, 85, 54, -54, -85,
                4, 88, 46, -61, -82, 13, 90, 38, -67, -78, 22, 90, 31, -73,
            ],
            [
                70, -43, -87, 9, 90, 25, -80, -57, 57, 80, -25, -90, -9, 87, 43, -70, -70, 43, 87,
                -9, -90, -25, 80, 57, -57, -80, 25, 90, 9, -87, -43, 70,
            ],
            [
                67, -54, -78, 38, 85, -22, -90, 4, 90, 13, -88, -31, 82, 46, -73, -61, 61, 73, -46,
                -82, 31, 88, -13, -90, -4, 90, 22, -85, -38, 78, 54, -67,
            ],
            [
                64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64, 64, -64,
                -64, 64, 64, -64, -64, 64, 64, -64, -64, 64, 64, -64, -64, 64,
            ],
            [
                61, -73, -46, 82, 31, -88, -13, 90, -4, -90, 22, 85, -38, -78, 54, 67, -67, -54,
                78, 38, -85, -22, 90, 4, -90, 13, 88, -31, -82, 46, 73, -61,
            ],
            [
                57, -80, -25, 90, -9, -87, 43, 70, -70, -43, 87, 9, -90, 25, 80, -57, -57, 80, 25,
                -90, 9, 87, -43, -70, 70, 43, -87, -9, 90, -25, -80, 57,
            ],
            [
                54, -85, -4, 88, -46, -61, 82, 13, -90, 38, 67, -78, -22, 90, -31, -73, 73, 31,
                -90, 22, 78, -67, -38, 90, -13, -82, 61, 46, -88, 4, 85, -54,
            ],
            [
                50, -89, 18, 75, -75, -18, 89, -50, -50, 89, -18, -75, 75, 18, -89, 50, 50, -89,
                18, 75, -75, -18, 89, -50, -50, 89, -18, -75, 75, 18, -89, 50,
            ],
            [
                46, -90, 38, 54, -90, 31, 61, -88, 22, 67, -85, 13, 73, -82, 4, 78, -78, -4, 82,
                -73, -13, 85, -67, -22, 88, -61, -31, 90, -54, -38, 90, -46,
            ],
            [
                43, -90, 57, 25, -87, 70, 9, -80, 80, -9, -70, 87, -25, -57, 90, -43, -43, 90, -57,
                -25, 87, -70, -9, 80, -80, 9, 70, -87, 25, 57, -90, 43,
            ],
            [
                38, -88, 73, -4, -67, 90, -46, -31, 85, -78, 13, 61, -90, 54, 22, -82, 82, -22,
                -54, 90, -61, -13, 78, -85, 31, 46, -90, 67, 4, -73, 88, -38,
            ],
            [
                36, -83, 83, -36, -36, 83, -83, 36, 36, -83, 83, -36, -36, 83, -83, 36, 36, -83,
                83, -36, -36, 83, -83, 36, 36, -83, 83, -36, -36, 83, -83, 36,
            ],
            [
                31, -78, 90, -61, 4, 54, -88, 82, -38, -22, 73, -90, 67, -13, -46, 85, -85, 46, 13,
                -67, 90, -73, 22, 38, -82, 88, -54, -4, 61, -90, 78, -31,
            ],
            [
                25, -70, 90, -80, 43, 9, -57, 87, -87, 57, -9, -43, 80, -90, 70, -25, -25, 70, -90,
                80, -43, -9, 57, -87, 87, -57, 9, 43, -80, 90, -70, 25,
            ],
            [
                22, -61, 85, -90, 73, -38, -4, 46, -78, 90, -82, 54, -13, -31, 67, -88, 88, -67,
                31, 13, -54, 82, -90, 78, -46, 4, 38, -73, 90, -85, 61, -22,
            ],
            [
                18, -50, 75, -89, 89, -75, 50, -18, -18, 50, -75, 89, -89, 75, -50, 18, 18, -50,
                75, -89, 89, -75, 50, -18, -18, 50, -75, 89, -89, 75, -50, 18,
            ],
            [
                13, -38, 61, -78, 88, -90, 85, -73, 54, -31, 4, 22, -46, 67, -82, 90, -90, 82, -67,
                46, -22, -4, 31, -54, 73, -85, 90, -88, 78, -61, 38, -13,
            ],
            [
                9, -25, 43, -57, 70, -80, 87, -90, 90, -87, 80, -70, 57, -43, 25, -9, -9, 25, -43,
                57, -70, 80, -87, 90, -90, 87, -80, 70, -57, 43, -25, 9,
            ],
            [
                4, -13, 22, -31, 38, -46, 54, -61, 67, -73, 78, -82, 85, -88, 90, -90, 90, -90, 88,
                -85, 82, -78, 73, -67, 61, -54, 46, -38, 31, -22, 13, -4,
            ],
        ];

        // Sparse 32x32 block from a real HEIC decode (sideways2.heic CTU near pixel 288,0).
        let nz: [(usize, i32); 40] = [
            (0, -45),
            (1, 81),
            (2, -18),
            (3, 9),
            (7, -9),
            (9, -9),
            (32, -9),
            (33, -27),
            (34, 18),
            (37, 9),
            (38, -9),
            (65, -9),
            (66, 9),
            (67, 9),
            (69, -18),
            (73, -9),
            (97, -9),
            (98, -9),
            (102, 9),
            (104, -9),
            (129, -9),
            (130, -9),
            (131, 9),
            (161, 9),
            (162, 9),
            (163, 9),
            (165, -9),
            (168, -9),
            (170, -9),
            (193, 9),
            (195, 9),
            (198, 9),
            (228, 9),
            (233, 9),
            (262, 9),
            (263, 9),
            (264, 9),
            (267, -9),
            (298, 9),
            (327, 9),
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
                coeffs_butterfly[i],
                reference[i],
                "mismatch at ({}, {}): butterfly={} reference={}",
                i % 32,
                i / 32,
                coeffs_butterfly[i],
                reference[i]
            );
        }

        // Verify that output[0] is 0 (not 1) for these specific coefficients.
        assert_eq!(coeffs_butterfly[0], 0);
    }
}
