/// Native CPU image operations on RGB8 data (3 bytes per pixel, row-major).
///
/// Replaces `image::imageops` for flip, rotate, and resize operations.
use rayon::prelude::*;
use std::f64::consts::PI;

const BPP: usize = 3; // bytes per pixel (RGB8)

/// Reverse pixel order within each row.
pub fn flip_horizontal(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    let stride = w * BPP;
    let mut out = vec![0u8; rgb.len()];

    for y in 0..h {
        let row_start = y * stride;
        for x in 0..w {
            let src = row_start + x * BPP;
            let dst = row_start + (w - 1 - x) * BPP;
            out[dst..dst + BPP].copy_from_slice(&rgb[src..src + BPP]);
        }
    }
    out
}

/// Reverse the order of rows.
pub fn flip_vertical(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    let stride = w * BPP;
    let mut out = vec![0u8; rgb.len()];

    for y in 0..h {
        let src_start = y * stride;
        let dst_start = (h - 1 - y) * stride;
        out[dst_start..dst_start + stride].copy_from_slice(&rgb[src_start..src_start + stride]);
    }
    out
}

/// Rotate 90 degrees clockwise. Returns (data, new_width, new_height).
///
/// Output pixel at (x, y) in new image comes from input pixel at (y, h-1-x).
/// New dimensions: width=h, height=w.
pub fn rotate90(rgb: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let new_w = h;
    let new_h = w;
    let mut out = vec![0u8; rgb.len()];

    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * BPP;
            // dst position: new image coords (dst_x, dst_y) = (h-1-y mapped...)
            // output(x', y') pulls from input(y', h-1-x')
            // so input(y, x) goes to output(h-1-y, x) ... let's derive:
            // out(dst_x, dst_y) = in(dst_y, h-1-dst_x)
            // We want: in(y, x) -> which (dst_x, dst_y)?
            // dst_y = y, h-1-dst_x = x => dst_x = h-1-x... wait.
            // Let me re-derive: for CW 90, input(col, row) -> output(h-1-row, col)
            // So input pixel at (x, y) goes to output pixel at (h-1-y, x)
            let dst_x = h - 1 - y;
            let dst_y = x;
            let dst = (dst_y * new_w + dst_x) * BPP;
            out[dst..dst + BPP].copy_from_slice(&rgb[src..src + BPP]);
        }
    }
    (out, new_w, new_h)
}

/// Rotate 180 degrees. Same dimensions.
pub fn rotate180(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    let total_pixels = w * h;
    let mut out = vec![0u8; rgb.len()];

    for i in 0..total_pixels {
        let src = i * BPP;
        let dst = (total_pixels - 1 - i) * BPP;
        out[dst..dst + BPP].copy_from_slice(&rgb[src..src + BPP]);
    }
    out
}

/// Rotate 270 degrees clockwise (= 90 CCW). Returns (data, new_width, new_height).
///
/// Output pixel at (x, y) in new image comes from input pixel at (w-1-y, x).
/// New dimensions: width=h, height=w.
pub fn rotate270(rgb: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let new_w = h;
    let new_h = w;
    let mut out = vec![0u8; rgb.len()];

    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * BPP;
            // CW 270 = CCW 90: input(x, y) -> output(y, w-1-x)
            let dst_x = y;
            let dst_y = w - 1 - x;
            let dst = (dst_y * new_w + dst_x) * BPP;
            out[dst..dst + BPP].copy_from_slice(&rgb[src..src + BPP]);
        }
    }
    (out, new_w, new_h)
}

/// Two-pass separable Lanczos3 resize on RGB8 data.
///
/// Pass 1: horizontal resize (src_w -> dst_w).
/// Pass 2: vertical resize on intermediate result.
pub fn resize_lanczos3(
    rgb: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<u8> {
    if dst_w == 0 || dst_h == 0 {
        return Vec::new();
    }

    // Pass 1: horizontal
    let intermediate = resize_horizontal(rgb, src_w, src_h, dst_w);

    // Pass 2: vertical
    resize_vertical(&intermediate, dst_w, src_h, dst_h)
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = PI * x;
        px.sin() / px
    }
}

fn lanczos3_kernel(x: f64) -> f64 {
    let ax = x.abs();
    if ax < 3.0 {
        sinc(x) * sinc(x / 3.0)
    } else {
        0.0
    }
}

fn resize_horizontal(src: &[u8], src_w: usize, src_h: usize, dst_w: usize) -> Vec<u8> {
    let dst_stride = dst_w * BPP;
    let src_stride = src_w * BPP;
    let mut out = vec![0u8; dst_w * src_h * BPP];
    let ratio = src_w as f64 / dst_w as f64;
    // When downscaling, widen the kernel by the scale ratio so it covers
    // enough source pixels to act as an anti-aliasing low-pass filter.
    let filter_scale = ratio.max(1.0);
    let support = 3.0 * filter_scale;

    out.par_chunks_mut(dst_stride)
        .enumerate()
        .for_each(|(y, dst_row_buf)| {
            let src_row = y * src_stride;

            for dx in 0..dst_w {
                let center = (dx as f64 + 0.5) * ratio - 0.5;
                let left = (center - support).ceil() as i64;
                let right = (center + support).floor() as i64;

                let mut sum_r = 0.0_f64;
                let mut sum_g = 0.0_f64;
                let mut sum_b = 0.0_f64;
                let mut weight_sum = 0.0_f64;

                for sx in left..=right {
                    let clamped = sx.clamp(0, src_w as i64 - 1) as usize;
                    let w = lanczos3_kernel((sx as f64 - center) / filter_scale);
                    let off = src_row + clamped * BPP;
                    sum_r += src[off] as f64 * w;
                    sum_g += src[off + 1] as f64 * w;
                    sum_b += src[off + 2] as f64 * w;
                    weight_sum += w;
                }

                let inv = if weight_sum.abs() > 1e-12 {
                    1.0 / weight_sum
                } else {
                    0.0
                };
                let off = dx * BPP;
                dst_row_buf[off] = (sum_r * inv).round().clamp(0.0, 255.0) as u8;
                dst_row_buf[off + 1] = (sum_g * inv).round().clamp(0.0, 255.0) as u8;
                dst_row_buf[off + 2] = (sum_b * inv).round().clamp(0.0, 255.0) as u8;
            }
        });

    out
}

fn resize_vertical(src: &[u8], width: usize, src_h: usize, dst_h: usize) -> Vec<u8> {
    let stride = width * BPP;
    let mut out = vec![0u8; width * dst_h * BPP];
    let ratio = src_h as f64 / dst_h as f64;
    let filter_scale = ratio.max(1.0);
    let support = 3.0 * filter_scale;

    out.par_chunks_mut(stride)
        .enumerate()
        .for_each(|(dy, dst_row_buf)| {
            let center = (dy as f64 + 0.5) * ratio - 0.5;
            let top = (center - support).ceil() as i64;
            let bottom = (center + support).floor() as i64;

            let mut weights: Vec<(usize, f64)> = Vec::with_capacity((bottom - top + 1) as usize);
            let mut weight_sum = 0.0_f64;
            for sy in top..=bottom {
                let clamped = sy.clamp(0, src_h as i64 - 1) as usize;
                let w = lanczos3_kernel((sy as f64 - center) / filter_scale);
                weights.push((clamped, w));
                weight_sum += w;
            }
            let inv = if weight_sum.abs() > 1e-12 {
                1.0 / weight_sum
            } else {
                0.0
            };

            for x in 0..width {
                let mut sum_r = 0.0_f64;
                let mut sum_g = 0.0_f64;
                let mut sum_b = 0.0_f64;
                for &(sy, w) in &weights {
                    let off = sy * stride + x * BPP;
                    sum_r += src[off] as f64 * w;
                    sum_g += src[off + 1] as f64 * w;
                    sum_b += src[off + 2] as f64 * w;
                }
                let off = x * BPP;
                dst_row_buf[off] = (sum_r * inv).round().clamp(0.0, 255.0) as u8;
                dst_row_buf[off + 1] = (sum_g * inv).round().clamp(0.0, 255.0) as u8;
                dst_row_buf[off + 2] = (sum_b * inv).round().clamp(0.0, 255.0) as u8;
            }
        });

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: create a simple RGB image with each pixel = (x, y, channel) for easy verification
    fn make_test_image(w: usize, h: usize) -> Vec<u8> {
        let mut data = vec![0u8; w * h * BPP];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * BPP;
                data[off] = x as u8; // R = column
                data[off + 1] = y as u8; // G = row
                data[off + 2] = 128; // B = constant
            }
        }
        data
    }

    fn pixel_at(data: &[u8], w: usize, x: usize, y: usize) -> (u8, u8, u8) {
        let off = (y * w + x) * BPP;
        (data[off], data[off + 1], data[off + 2])
    }

    #[test]
    fn flip_horizontal_identity_1x1() {
        let img = vec![10, 20, 30];
        let result = flip_horizontal(&img, 1, 1);
        assert_eq!(result, vec![10, 20, 30]);
    }

    #[test]
    fn flip_horizontal_swaps_columns() {
        // 3x2 image: pixel (x,y) has R=x, G=y, B=128
        let img = make_test_image(3, 2);
        let result = flip_horizontal(&img, 3, 2);

        // After horizontal flip, column 0 becomes column 2, etc.
        assert_eq!(pixel_at(&result, 3, 0, 0), (2, 0, 128)); // was (2,0)
        assert_eq!(pixel_at(&result, 3, 1, 0), (1, 0, 128)); // was (1,0)
        assert_eq!(pixel_at(&result, 3, 2, 0), (0, 0, 128)); // was (0,0)
        assert_eq!(pixel_at(&result, 3, 0, 1), (2, 1, 128)); // was (2,1)
    }

    #[test]
    fn flip_vertical_swaps_rows() {
        let img = make_test_image(3, 2);
        let result = flip_vertical(&img, 3, 2);

        // Row 0 and row 1 are swapped
        assert_eq!(pixel_at(&result, 3, 0, 0), (0, 1, 128)); // was row 1
        assert_eq!(pixel_at(&result, 3, 0, 1), (0, 0, 128)); // was row 0
        assert_eq!(pixel_at(&result, 3, 2, 0), (2, 1, 128));
    }

    #[test]
    fn rotate90_cw_2x3() {
        // 2 wide x 3 tall image
        let img = make_test_image(2, 3);
        let (result, new_w, new_h) = rotate90(&img, 2, 3);

        assert_eq!(new_w, 3); // h becomes new width
        assert_eq!(new_h, 2); // w becomes new height

        // CW90: input(x, y) -> output(h-1-y, x) where h=3
        // input(0,0) -> output(2, 0): R=0, G=0
        assert_eq!(pixel_at(&result, 3, 2, 0), (0, 0, 128));
        // input(1,0) -> output(2, 1): R=1, G=0
        assert_eq!(pixel_at(&result, 3, 2, 1), (1, 0, 128));
        // input(0,1) -> output(1, 0): R=0, G=1
        assert_eq!(pixel_at(&result, 3, 1, 0), (0, 1, 128));
        // input(0,2) -> output(0, 0): R=0, G=2
        assert_eq!(pixel_at(&result, 3, 0, 0), (0, 2, 128));
        // input(1,2) -> output(0, 1): R=1, G=2
        assert_eq!(pixel_at(&result, 3, 0, 1), (1, 2, 128));
    }

    #[test]
    fn rotate180_reverses_all() {
        let img = make_test_image(3, 2);
        let result = rotate180(&img, 3, 2);

        // First pixel becomes last, etc.
        // input(0,0) -> output(2,1)
        assert_eq!(pixel_at(&result, 3, 2, 1), (0, 0, 128));
        // input(2,1) -> output(0,0)
        assert_eq!(pixel_at(&result, 3, 0, 0), (2, 1, 128));
    }

    #[test]
    fn rotate270_ccw_2x3() {
        let img = make_test_image(2, 3);
        let (result, new_w, new_h) = rotate270(&img, 2, 3);

        assert_eq!(new_w, 3);
        assert_eq!(new_h, 2);

        // CCW90: input(x, y) -> output(y, w-1-x) where w=2
        // input(0,0) -> output(0, 1)
        assert_eq!(pixel_at(&result, 3, 0, 1), (0, 0, 128));
        // input(1,0) -> output(0, 0)
        assert_eq!(pixel_at(&result, 3, 0, 0), (1, 0, 128));
        // input(0,2) -> output(2, 1)
        assert_eq!(pixel_at(&result, 3, 2, 1), (0, 2, 128));
        // input(1,2) -> output(2, 0)
        assert_eq!(pixel_at(&result, 3, 2, 0), (1, 2, 128));
    }

    #[test]
    fn rotate90_then_rotate270_is_identity() {
        let img = make_test_image(4, 3);
        let (r90, w1, h1) = rotate90(&img, 4, 3);
        let (roundtrip, w2, h2) = rotate270(&r90, w1, h1);
        assert_eq!(w2, 4);
        assert_eq!(h2, 3);
        assert_eq!(roundtrip, img);
    }

    #[test]
    fn rotate180_is_involution() {
        let img = make_test_image(4, 3);
        let r180 = rotate180(&img, 4, 3);
        let roundtrip = rotate180(&r180, 4, 3);
        assert_eq!(roundtrip, img);
    }

    #[test]
    fn flip_horizontal_is_involution() {
        let img = make_test_image(4, 3);
        let flipped = flip_horizontal(&img, 4, 3);
        let roundtrip = flip_horizontal(&flipped, 4, 3);
        assert_eq!(roundtrip, img);
    }

    #[test]
    fn flip_vertical_is_involution() {
        let img = make_test_image(4, 3);
        let flipped = flip_vertical(&img, 4, 3);
        let roundtrip = flip_vertical(&flipped, 4, 3);
        assert_eq!(roundtrip, img);
    }

    #[test]
    fn resize_identity_dimensions() {
        // Resizing to same dimensions should preserve pixel values approximately
        let img = make_test_image(4, 4);
        let result = resize_lanczos3(&img, 4, 4, 4, 4);
        assert_eq!(result.len(), 4 * 4 * BPP);
        // Lanczos identity should be very close to original
        for i in 0..result.len() {
            let diff = (result[i] as i32 - img[i] as i32).unsigned_abs();
            assert!(
                diff <= 1,
                "pixel byte {i}: got {}, expected {}",
                result[i],
                img[i]
            );
        }
    }

    #[test]
    fn resize_output_dimensions() {
        let img = make_test_image(10, 8);
        let result = resize_lanczos3(&img, 10, 8, 5, 4);
        assert_eq!(result.len(), 5 * 4 * BPP);
    }

    #[test]
    fn resize_upscale_dimensions() {
        let img = make_test_image(2, 2);
        let result = resize_lanczos3(&img, 2, 2, 6, 6);
        assert_eq!(result.len(), 6 * 6 * BPP);
    }

    #[test]
    fn resize_empty_output() {
        let img = make_test_image(4, 4);
        assert!(resize_lanczos3(&img, 4, 4, 0, 0).is_empty());
        assert!(resize_lanczos3(&img, 4, 4, 0, 4).is_empty());
        assert!(resize_lanczos3(&img, 4, 4, 4, 0).is_empty());
    }

    #[test]
    fn resize_solid_color_stays_solid() {
        // A uniform color image should remain uniform after resize
        let w = 8;
        let h = 8;
        let img: Vec<u8> = vec![100, 150, 200].repeat(w * h);
        let result = resize_lanczos3(&img, w, h, 4, 4);

        for chunk in result.chunks_exact(BPP) {
            let diff_r = (chunk[0] as i32 - 100).unsigned_abs();
            let diff_g = (chunk[1] as i32 - 150).unsigned_abs();
            let diff_b = (chunk[2] as i32 - 200).unsigned_abs();
            assert!(
                diff_r <= 1 && diff_g <= 1 && diff_b <= 1,
                "Expected ~(100,150,200), got ({},{},{})",
                chunk[0],
                chunk[1],
                chunk[2]
            );
        }
    }

    #[test]
    fn sinc_at_zero() {
        assert!((sinc(0.0) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn lanczos3_kernel_zero_beyond_3() {
        assert_eq!(lanczos3_kernel(3.0), 0.0);
        assert_eq!(lanczos3_kernel(-3.0), 0.0);
        assert_eq!(lanczos3_kernel(5.0), 0.0);
    }

    #[test]
    fn lanczos3_kernel_at_zero() {
        assert!((lanczos3_kernel(0.0) - 1.0).abs() < 1e-10);
    }
}
