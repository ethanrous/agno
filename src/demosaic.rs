use rayon::{
    iter::{IndexedParallelIterator, ParallelIterator},
    slice::ParallelSliceMut,
};

// Minimal dependencies: adjust imports/types to your crate as needed.
use crate::sony_decoder::Dimensions;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BayerPattern {
    RGGB,
    // BGGR,
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
        // BayerPattern::BGGR => match (r, c) {
        //     (0, 0) => CfaColor::B,
        //     (0, 1) => CfaColor::G,
        //     (1, 0) => CfaColor::G,
        //     _ => CfaColor::R,
        // },
        // BayerPattern::GRBG => match (r, c) {
        //     (0, 0) => CfaColor::G,
        //     (0, 1) => CfaColor::R,
        //     (1, 0) => CfaColor::B,
        //     _ => CfaColor::G,
        // },
        // BayerPattern::GBRG => match (r, c) {
        //     (0, 0) => CfaColor::G,
        //     (0, 1) => CfaColor::B,
        //     (1, 0) => CfaColor::R,
        //     _ => CfaColor::G,
        // },
    }
}

#[inline(always)]
fn tone_u8(v: f32, gamma: f32) -> u8 {
    // v is linear and roughly in [0,1], but may exceed 1 slightly after WB
    let n = v.max(0.0).min(1.0);
    (n.powf(1.0 / gamma.max(0.001)) * 255.0 + 0.5).floor() as u8
}

// Returns the black-subtracted, normalized, WB-scaled value at (y,x) as f32 (linear).
#[inline(always)]
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

/// Compact bilinear demosaic with WB applied BEFORE interpolation.
/// - raw: u16 mosaic buffer with stride dims.raw_width
/// - dims.output_width/height are the image dimensions you want to render
/// - white_level should be the sensor’s maximum code value (e.g., 0x3FFF for 14-bit)
/// - wb: gains [R,G,B], e.g. from AsShotNeutral or a gray-world estimate
pub fn demosaic_bilinear_to_rgb8(
    raw: &[u16],
    dims: Dimensions,
    pattern: BayerPattern,
    black_level: u16,
    white_level: u16,
    wb: [f32; 3],
    gamma: f32,
) -> Vec<u8> {
    let w = dims.output_width;
    let h = dims.output_height;
    let stride = dims.raw_width;

    // Normalization (after black subtraction)
    let range = white_level.saturating_sub(black_level).max(1) as f32;
    let inv_range = 1.0 / range;

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

                // Clamp negatives that might occur from WB/matrix later (if you add one)
                r = r.max(0.0);
                g = g.max(0.0);
                b = b.max(0.0);

                let o = x * 3;
                out_row[o] = tone_u8(r, gamma);
                out_row[o + 1] = tone_u8(g, gamma);
                out_row[o + 2] = tone_u8(b, gamma);
            }
        });

    out
}
