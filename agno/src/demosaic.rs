use rayon::{
    iter::{IndexedParallelIterator, ParallelIterator},
    slice::ParallelSliceMut,
};

// Minimal dependencies: adjust imports/types to your crate as needed.
use crate::sony_decoder::Dimensions;

// Import shared constants when GPU feature is enabled
#[cfg(feature = "gpu")]
use agno_gpu_shared::{EXPOSURE_EV, SATURATION};

// "Camera look" defaults - hardcoded for non-GPU builds
#[cfg(not(feature = "gpu"))]
const EXPOSURE_EV: f32 = 1.2; // Sony RAW significantly underexposed
#[cfg(not(feature = "gpu"))]
const SATURATION: f32 = 1.25; // 25% saturation boost

#[allow(dead_code, clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BayerPattern {
    RGGB, // Sony and Canon sensors
    BGGR, // Some other cameras
          // GRBG,
          // GBRG,
}

#[inline(always)]
fn clamp_i32(x: i32, lo: i32, hi: i32) -> i32 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

#[inline(always)]
fn idx(row: usize, col: usize, stride: usize) -> usize {
    row * stride + col
}

#[derive(Clone, Copy, PartialEq)]
enum CfaColor {
    R,
    G,
    B,
}

#[inline(always)]
fn cfa_color_at(row: usize, col: usize, pattern: BayerPattern) -> CfaColor {
    let r = row & 1;
    let c = col & 1;
    match pattern {
        BayerPattern::RGGB => match (r, c) {
            (0, 0) => CfaColor::R,
            (0, 1) => CfaColor::G,
            (1, 0) => CfaColor::G,
            _ => CfaColor::B,
        },
        BayerPattern::BGGR => match (r, c) {
            (0, 0) => CfaColor::B,
            (0, 1) => CfaColor::G,
            (1, 0) => CfaColor::G,
            _ => CfaColor::R,
        },
    }
}

/// Calculate luminance using Rec. 709 coefficients
#[inline(always)]
fn luminance(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Apply saturation adjustment using luminance-preserving interpolation
#[inline(always)]
fn apply_saturation(r: f32, g: f32, b: f32, saturation: f32) -> (f32, f32, f32) {
    let lum = luminance(r, g, b);
    (
        lum + (r - lum) * saturation,
        lum + (g - lum) * saturation,
        lum + (b - lum) * saturation,
    )
}

/// Apply shadow-lifting tone curve using a power function
/// Values < 1.0 lift shadows, values > 1.0 darken them
#[inline(always)]
fn apply_tone_curve(v: f32) -> f32 {
    // Use x^0.85 to lift shadows - this maps low values higher
    // e.g., 0.1^0.85 ≈ 0.14, 0.2^0.85 ≈ 0.26, 0.5^0.85 ≈ 0.55
    v.max(0.0).powf(0.85).min(1.0)
}

#[inline(always)]
fn tone_u8(v: f32, gamma: f32) -> u8 {
    let curved = apply_tone_curve(v);
    (curved.powf(1.0 / gamma.max(0.001)) * 255.0 + 0.5).floor() as u8
}

// Returns the black-subtracted, normalized, WB-scaled value at (y,x) as f32 (linear).
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn sample_wb(
    raw: &[u16],
    row: usize,
    col: usize,
    stride: usize,
    pattern: BayerPattern,
    black: u16,
    inv_range: f32,
    wb: [f32; 3],
) -> f32 {
    let v = raw[idx(row, col, stride)].saturating_sub(black) as f32 * inv_range;
    let gain = match cfa_color_at(row, col, pattern) {
        CfaColor::R => wb[0],
        CfaColor::G => wb[1],
        CfaColor::B => wb[2],
    };
    v * gain
}

/// Apply a 3x3 color correction matrix to RGB values.
/// Matrix is stored row-major: [r0, r1, r2, g0, g1, g2, b0, b1, b2]
/// R_out = m[0]*R + m[1]*G + m[2]*B
/// G_out = m[3]*R + m[4]*G + m[5]*B
/// B_out = m[6]*R + m[7]*G + m[8]*B
#[inline(always)]
fn apply_color_matrix(r: f32, g: f32, b: f32, m: &[f32; 9]) -> (f32, f32, f32) {
    (
        m[0] * r + m[1] * g + m[2] * b,
        m[3] * r + m[4] * g + m[5] * b,
        m[6] * r + m[7] * g + m[8] * b,
    )
}

/// Compact bilinear demosaic with WB applied BEFORE interpolation.
/// - raw: u16 mosaic buffer with stride dims.raw_width
/// - dims.output_width/height are the image dimensions you want to render
/// - white_level should be the sensor's maximum code value (e.g., 0x3FFF for 14-bit)
/// - wb: gains [R,G,B], e.g. from AsShotNeutral or a gray-world estimate
/// - color_matrix: 3x3 camera-to-XYZ color correction matrix (row-major)
///
/// When the `gpu` feature is enabled and a GPU is available, this function
/// uses GPU-accelerated processing. Otherwise, it falls back to CPU (Rayon).
#[allow(clippy::too_many_arguments)]
pub fn demosaic_bilinear_to_rgb8(
    raw: &[u16],
    dims: Dimensions,
    pattern: BayerPattern,
    black_level: u16,
    white_level: u16,
    wb: [f32; 3],
    color_matrix: [f32; 9],
    gamma: f32,
) -> Vec<u8> {
    tracing::debug!("Checking GPU availability");

    // Try GPU path first if feature is enabled
    #[cfg(feature = "gpu")]
    if let Some(result) = crate::demosaic_gpu::demosaic_bilinear_to_rgb8_gpu(
        raw,
        dims,
        pattern,
        black_level,
        white_level,
        wb,
        color_matrix,
        gamma,
    ) {
        tracing::debug!("Using GPU-accelerated demosaic");
        return result;
    }

    #[cfg(feature = "gpu")]
    tracing::debug!("GPU unavailable, falling back to CPU demosaic");

    demosaic_bilinear_to_rgb8_cpu(
        raw,
        dims,
        pattern,
        black_level,
        white_level,
        wb,
        color_matrix,
        gamma,
    )
}

/// CPU implementation of bilinear demosaic using Rayon for parallelization.
#[allow(clippy::too_many_arguments)]
fn demosaic_bilinear_to_rgb8_cpu(
    raw: &[u16],
    dims: Dimensions,
    pattern: BayerPattern,
    black_level: u16,
    white_level: u16,
    wb: [f32; 3],
    color_matrix: [f32; 9],
    gamma: f32,
) -> Vec<u8> {
    let w = dims.output_width;
    let h = dims.output_height;
    let stride = dims.raw_width;

    // Normalization (after black subtraction)
    let range = white_level.saturating_sub(black_level).max(1) as f32;
    let inv_range = 1.0 / range;

    // Pre-compute exposure multiplier (2^EV)
    let exposure_mult = 2.0_f32.powf(EXPOSURE_EV);

    let mut out = vec![0u8; w * h * 3];

    out.par_chunks_mut(w * 3)
        .enumerate()
        .for_each(|(row, out_row)| {
            let y0 = clamp_i32(row as i32 - 1, 0, (h - 1) as i32) as usize;
            let y2 = clamp_i32(row as i32 + 1, 0, (h - 1) as i32) as usize;

            for x in 0..w {
                let x0 = clamp_i32(x as i32 - 1, 0, (w - 1) as i32) as usize;
                let x2 = clamp_i32(x as i32 + 1, 0, (w - 1) as i32) as usize;

                // WB applied per mosaic sample (pre-demosaic)
                let here = sample_wb(raw, row, x, stride, pattern, black_level, inv_range, wb);
                let up = sample_wb(raw, y0, x, stride, pattern, black_level, inv_range, wb);
                let down = sample_wb(raw, y2, x, stride, pattern, black_level, inv_range, wb);
                let left = sample_wb(raw, row, x0, stride, pattern, black_level, inv_range, wb);
                let right = sample_wb(raw, row, x2, stride, pattern, black_level, inv_range, wb);

                // Bilinear interpolation with correct G orientation
                let (mut r, mut g, mut b) = match cfa_color_at(row, x, pattern) {
                    CfaColor::R => {
                        let ul =
                            sample_wb(raw, y0, x0, stride, pattern, black_level, inv_range, wb);
                        let ur =
                            sample_wb(raw, y0, x2, stride, pattern, black_level, inv_range, wb);
                        let dl =
                            sample_wb(raw, y2, x0, stride, pattern, black_level, inv_range, wb);
                        let dr =
                            sample_wb(raw, y2, x2, stride, pattern, black_level, inv_range, wb);

                        let g = (up + down + left + right) * 0.25;
                        let b = (ul + ur + dl + dr) * 0.25;
                        (here, g, b)
                    }
                    CfaColor::B => {
                        let ul =
                            sample_wb(raw, y0, x0, stride, pattern, black_level, inv_range, wb);
                        let ur =
                            sample_wb(raw, y0, x2, stride, pattern, black_level, inv_range, wb);
                        let dl =
                            sample_wb(raw, y2, x0, stride, pattern, black_level, inv_range, wb);
                        let dr =
                            sample_wb(raw, y2, x2, stride, pattern, black_level, inv_range, wb);

                        let g = (up + down + left + right) * 0.25;
                        let r = (ul + ur + dl + dr) * 0.25;
                        (r, g, here)
                    }
                    CfaColor::G => {
                        // Decide which color lies horizontally around this G site
                        let horiz = cfa_color_at(row, x ^ 1, pattern);
                        let r_val_h = (left + right) * 0.5;
                        let r_val_v = (up + down) * 0.5;
                        let b_val_h = r_val_h; // same averages, different assignment
                        let b_val_v = r_val_v;
                        let r = if horiz == CfaColor::R {
                            r_val_h
                        } else {
                            r_val_v
                        };
                        let b = if horiz == CfaColor::B {
                            b_val_h
                        } else {
                            b_val_v
                        };
                        (r, here, b)
                    }
                };

                // Clamp negatives
                r = r.max(0.0);
                g = g.max(0.0);
                b = b.max(0.0);

                // Apply color correction matrix (camera RGB to standard RGB)
                let (r_cc, g_cc, b_cc) = apply_color_matrix(r, g, b, &color_matrix);
                r = r_cc.max(0.0);
                g = g_cc.max(0.0);
                b = b_cc.max(0.0);

                // Apply exposure compensation
                let r = r * exposure_mult;
                let g = g * exposure_mult;
                let b = b * exposure_mult;

                // Apply saturation boost (luminance-preserving)
                let (r, g, b) = apply_saturation(r, g, b, SATURATION);

                let o = x * 3;
                out_row[o] = tone_u8(r, gamma);
                out_row[o + 1] = tone_u8(g, gamma);
                out_row[o + 2] = tone_u8(b, gamma);
            }
        });

    out
}
