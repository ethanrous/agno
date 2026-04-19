/// Native CPU image operations on RGB8/RGBA8 data (3 or 4 bytes per pixel, row-major).
///
/// Replaces `image::imageops` for flip, rotate, and resize operations.
use rayon::prelude::*;
use std::f64::consts::PI;

/// Reverse pixel order within each row. `channels` = 3 (RGB) or 4 (RGBA).
pub fn flip_horizontal(src: &[u8], w: usize, h: usize, channels: usize) -> Vec<u8> {
    let stride = w * channels;
    let mut out = vec![0u8; src.len()];

    for y in 0..h {
        let row_start = y * stride;
        for x in 0..w {
            let s = row_start + x * channels;
            let d = row_start + (w - 1 - x) * channels;
            out[d..d + channels].copy_from_slice(&src[s..s + channels]);
        }
    }
    out
}

/// Reverse the order of rows.
pub fn flip_vertical(src: &[u8], w: usize, h: usize, channels: usize) -> Vec<u8> {
    let stride = w * channels;
    let mut out = vec![0u8; src.len()];

    for y in 0..h {
        let s = y * stride;
        let d = (h - 1 - y) * stride;
        out[d..d + stride].copy_from_slice(&src[s..s + stride]);
    }
    out
}

/// Rotate 90 degrees clockwise. Returns (data, new_width, new_height).
pub fn rotate90(src: &[u8], w: usize, h: usize, channels: usize) -> (Vec<u8>, usize, usize) {
    let new_w = h;
    let new_h = w;
    let mut out = vec![0u8; src.len()];

    for y in 0..h {
        for x in 0..w {
            let s = (y * w + x) * channels;
            let dst_x = h - 1 - y;
            let dst_y = x;
            let d = (dst_y * new_w + dst_x) * channels;
            out[d..d + channels].copy_from_slice(&src[s..s + channels]);
        }
    }
    (out, new_w, new_h)
}

/// Rotate 180 degrees. Same dimensions.
pub fn rotate180(src: &[u8], w: usize, h: usize, channels: usize) -> Vec<u8> {
    let total = w * h;
    let mut out = vec![0u8; src.len()];

    for i in 0..total {
        let s = i * channels;
        let d = (total - 1 - i) * channels;
        out[d..d + channels].copy_from_slice(&src[s..s + channels]);
    }
    out
}

/// Rotate 270 degrees clockwise (= 90 CCW). Returns (data, new_width, new_height).
pub fn rotate270(src: &[u8], w: usize, h: usize, channels: usize) -> (Vec<u8>, usize, usize) {
    let new_w = h;
    let new_h = w;
    let mut out = vec![0u8; src.len()];

    for y in 0..h {
        for x in 0..w {
            let s = (y * w + x) * channels;
            let dst_x = y;
            let dst_y = w - 1 - x;
            let d = (dst_y * new_w + dst_x) * channels;
            out[d..d + channels].copy_from_slice(&src[s..s + channels]);
        }
    }
    (out, new_w, new_h)
}

/// Two-pass separable Lanczos3 resize. `channels` = 3 (RGB) or 4 (RGBA).
pub fn resize_lanczos3(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    channels: usize,
) -> Vec<u8> {
    if dst_w == 0 || dst_h == 0 {
        return Vec::new();
    }
    let intermediate = resize_horizontal(src, src_w, src_h, dst_w, channels);
    resize_vertical(&intermediate, dst_w, src_h, dst_h, channels)
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

fn resize_horizontal(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    channels: usize,
) -> Vec<u8> {
    let dst_stride = dst_w * channels;
    let src_stride = src_w * channels;
    let mut out = vec![0u8; dst_w * src_h * channels];
    let ratio = src_w as f64 / dst_w as f64;
    // When downscaling, widen the kernel by the scale ratio so it covers
    // enough source pixels to act as an anti-aliasing low-pass filter.
    let filter_scale = ratio.max(1.0);
    let support = 3.0 * filter_scale;

    out.par_chunks_mut(dst_stride)
        .enumerate()
        .for_each(|(y, dst_row_buf)| {
            let src_row = y * src_stride;
            let mut sums = vec![0.0_f64; channels];

            for dx in 0..dst_w {
                let center = (dx as f64 + 0.5) * ratio - 0.5;
                let left = (center - support).ceil() as i64;
                let right = (center + support).floor() as i64;

                sums.iter_mut().for_each(|s| *s = 0.0);
                let mut weight_sum = 0.0_f64;

                for sx in left..=right {
                    let clamped = sx.clamp(0, src_w as i64 - 1) as usize;
                    let w = lanczos3_kernel((sx as f64 - center) / filter_scale);
                    let off = src_row + clamped * channels;
                    for c in 0..channels {
                        sums[c] += src[off + c] as f64 * w;
                    }
                    weight_sum += w;
                }

                let inv = if weight_sum.abs() > 1e-12 {
                    1.0 / weight_sum
                } else {
                    0.0
                };
                let off = dx * channels;
                for c in 0..channels {
                    dst_row_buf[off + c] = (sums[c] * inv).round().clamp(0.0, 255.0) as u8;
                }
            }
        });

    out
}

fn resize_vertical(
    src: &[u8],
    width: usize,
    src_h: usize,
    dst_h: usize,
    channels: usize,
) -> Vec<u8> {
    let stride = width * channels;
    let mut out = vec![0u8; width * dst_h * channels];
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

            let mut sums = vec![0.0_f64; channels];
            for x in 0..width {
                sums.iter_mut().for_each(|s| *s = 0.0);
                for &(sy, w) in &weights {
                    let off = sy * stride + x * channels;
                    for c in 0..channels {
                        sums[c] += src[off + c] as f64 * w;
                    }
                }
                let off = x * channels;
                for c in 0..channels {
                    dst_row_buf[off + c] = (sums[c] * inv).round().clamp(0.0, 255.0) as u8;
                }
            }
        });

    out
}

/// Compose `src` centered inside a `dst_w × dst_h` buffer pre-filled with `pad`.
/// If any axis of `src` exceeds the corresponding axis of `dst`, the overflow
/// is center-cropped. `channels` = 3 or 4; `pad.len()` must equal `channels`.
pub fn compose_center(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    pad: &[u8],
    channels: usize,
) -> Vec<u8> {
    assert_eq!(
        src.len(),
        src_w * src_h * channels,
        "compose_center: src length mismatch"
    );
    assert_eq!(
        pad.len(),
        channels,
        "compose_center: pad color length must equal channels"
    );

    let mut out = Vec::with_capacity(dst_w * dst_h * channels);
    for _ in 0..(dst_w * dst_h) {
        out.extend_from_slice(pad);
    }

    let copy_w = src_w.min(dst_w);
    let copy_h = src_h.min(dst_h);
    if copy_w == 0 || copy_h == 0 {
        return out;
    }

    let src_x0 = src_w.saturating_sub(dst_w) / 2;
    let src_y0 = src_h.saturating_sub(dst_h) / 2;
    let dst_x0 = dst_w.saturating_sub(src_w) / 2;
    let dst_y0 = dst_h.saturating_sub(src_h) / 2;

    let src_row_bytes = src_w * channels;
    let dst_row_bytes = dst_w * channels;
    let copy_bytes = copy_w * channels;

    for row in 0..copy_h {
        let s = (src_y0 + row) * src_row_bytes + src_x0 * channels;
        let d = (dst_y0 + row) * dst_row_bytes + dst_x0 * channels;
        out[d..d + copy_bytes].copy_from_slice(&src[s..s + copy_bytes]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_image(w: usize, h: usize) -> Vec<u8> {
        let mut data = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 3;
                data[off] = x as u8;
                data[off + 1] = y as u8;
                data[off + 2] = 128;
            }
        }
        data
    }

    fn pixel_at(data: &[u8], w: usize, x: usize, y: usize, channels: usize) -> Vec<u8> {
        let off = (y * w + x) * channels;
        data[off..off + channels].to_vec()
    }

    #[test]
    fn flip_horizontal_identity_1x1() {
        let img = vec![10, 20, 30];
        let result = flip_horizontal(&img, 1, 1, 3);
        assert_eq!(result, vec![10, 20, 30]);
    }

    #[test]
    fn flip_horizontal_swaps_columns() {
        let img = make_test_image(3, 2);
        let result = flip_horizontal(&img, 3, 2, 3);

        assert_eq!(pixel_at(&result, 3, 0, 0, 3), vec![2, 0, 128]);
        assert_eq!(pixel_at(&result, 3, 1, 0, 3), vec![1, 0, 128]);
        assert_eq!(pixel_at(&result, 3, 2, 0, 3), vec![0, 0, 128]);
        assert_eq!(pixel_at(&result, 3, 0, 1, 3), vec![2, 1, 128]);
    }

    #[test]
    fn flip_vertical_swaps_rows() {
        let img = make_test_image(3, 2);
        let result = flip_vertical(&img, 3, 2, 3);

        assert_eq!(pixel_at(&result, 3, 0, 0, 3), vec![0, 1, 128]);
        assert_eq!(pixel_at(&result, 3, 0, 1, 3), vec![0, 0, 128]);
        assert_eq!(pixel_at(&result, 3, 2, 0, 3), vec![2, 1, 128]);
    }

    #[test]
    fn rotate90_cw_2x3() {
        let img = make_test_image(2, 3);
        let (result, new_w, new_h) = rotate90(&img, 2, 3, 3);

        assert_eq!(new_w, 3);
        assert_eq!(new_h, 2);

        assert_eq!(pixel_at(&result, 3, 2, 0, 3), vec![0, 0, 128]);
        assert_eq!(pixel_at(&result, 3, 2, 1, 3), vec![1, 0, 128]);
        assert_eq!(pixel_at(&result, 3, 1, 0, 3), vec![0, 1, 128]);
        assert_eq!(pixel_at(&result, 3, 0, 0, 3), vec![0, 2, 128]);
        assert_eq!(pixel_at(&result, 3, 0, 1, 3), vec![1, 2, 128]);
    }

    #[test]
    fn rotate180_reverses_all() {
        let img = make_test_image(3, 2);
        let result = rotate180(&img, 3, 2, 3);

        assert_eq!(pixel_at(&result, 3, 2, 1, 3), vec![0, 0, 128]);
        assert_eq!(pixel_at(&result, 3, 0, 0, 3), vec![2, 1, 128]);
    }

    #[test]
    fn rotate270_ccw_2x3() {
        let img = make_test_image(2, 3);
        let (result, new_w, new_h) = rotate270(&img, 2, 3, 3);

        assert_eq!(new_w, 3);
        assert_eq!(new_h, 2);

        assert_eq!(pixel_at(&result, 3, 0, 1, 3), vec![0, 0, 128]);
        assert_eq!(pixel_at(&result, 3, 0, 0, 3), vec![1, 0, 128]);
        assert_eq!(pixel_at(&result, 3, 2, 1, 3), vec![0, 2, 128]);
        assert_eq!(pixel_at(&result, 3, 2, 0, 3), vec![1, 2, 128]);
    }

    #[test]
    fn rotate90_then_rotate270_is_identity() {
        let img = make_test_image(4, 3);
        let (r90, w1, h1) = rotate90(&img, 4, 3, 3);
        let (roundtrip, w2, h2) = rotate270(&r90, w1, h1, 3);
        assert_eq!(w2, 4);
        assert_eq!(h2, 3);
        assert_eq!(roundtrip, img);
    }

    #[test]
    fn rotate180_is_involution() {
        let img = make_test_image(4, 3);
        let r180 = rotate180(&img, 4, 3, 3);
        let roundtrip = rotate180(&r180, 4, 3, 3);
        assert_eq!(roundtrip, img);
    }

    #[test]
    fn flip_horizontal_is_involution() {
        let img = make_test_image(4, 3);
        let flipped = flip_horizontal(&img, 4, 3, 3);
        let roundtrip = flip_horizontal(&flipped, 4, 3, 3);
        assert_eq!(roundtrip, img);
    }

    #[test]
    fn flip_vertical_is_involution() {
        let img = make_test_image(4, 3);
        let flipped = flip_vertical(&img, 4, 3, 3);
        let roundtrip = flip_vertical(&flipped, 4, 3, 3);
        assert_eq!(roundtrip, img);
    }

    #[test]
    fn resize_identity_dimensions() {
        let img = make_test_image(4, 4);
        let result = resize_lanczos3(&img, 4, 4, 4, 4, 3);
        assert_eq!(result.len(), 4 * 4 * 3);
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
        let result = resize_lanczos3(&img, 10, 8, 5, 4, 3);
        assert_eq!(result.len(), 5 * 4 * 3);
    }

    #[test]
    fn resize_upscale_dimensions() {
        let img = make_test_image(2, 2);
        let result = resize_lanczos3(&img, 2, 2, 6, 6, 3);
        assert_eq!(result.len(), 6 * 6 * 3);
    }

    #[test]
    fn resize_empty_output() {
        let img = make_test_image(4, 4);
        assert!(resize_lanczos3(&img, 4, 4, 0, 0, 3).is_empty());
        assert!(resize_lanczos3(&img, 4, 4, 0, 4, 3).is_empty());
        assert!(resize_lanczos3(&img, 4, 4, 4, 0, 3).is_empty());
    }

    #[test]
    fn resize_solid_color_stays_solid() {
        let w = 8;
        let h = 8;
        let img: Vec<u8> = vec![100, 150, 200].repeat(w * h);
        let result = resize_lanczos3(&img, w, h, 4, 4, 3);

        for chunk in result.chunks_exact(3) {
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

    #[test]
    fn flip_horizontal_rgba_swaps_columns() {
        let img = vec![10, 20, 30, 40, 50, 60, 70, 80];
        let out = flip_horizontal(&img, 2, 1, 4);
        assert_eq!(out, vec![50, 60, 70, 80, 10, 20, 30, 40]);
    }

    #[test]
    fn rotate90_rgba_preserves_alpha() {
        let img = vec![1, 2, 3, 40, 5, 6, 7, 80];
        let (out, new_w, new_h) = rotate90(&img, 2, 1, 4);
        assert_eq!((new_w, new_h), (1, 2));
        assert_eq!(&out[0..4], &[1, 2, 3, 40]);
        assert_eq!(&out[4..8], &[5, 6, 7, 80]);
    }

    #[test]
    fn resize_lanczos3_rgba_solid_stays_solid() {
        let img: Vec<u8> = [100, 150, 200, 128].repeat(16);
        let out = resize_lanczos3(&img, 4, 4, 2, 2, 4);
        assert_eq!(out.len(), 2 * 2 * 4);
        for chunk in out.chunks_exact(4) {
            for (i, (&got, &want)) in chunk.iter().zip(&[100, 150, 200, 128]).enumerate() {
                let diff = (got as i32 - want as i32).unsigned_abs();
                assert!(diff <= 1, "channel {i}: got {got}, want {want}");
            }
        }
    }

    #[test]
    fn compose_center_rgb_fills_corners_with_pad_color() {
        let src: Vec<u8> = vec![255, 0, 0].repeat(4);
        let out = compose_center(&src, 2, 2, 4, 4, &[0, 255, 0], 3);
        assert_eq!(out.len(), 4 * 4 * 3);
        assert_eq!(pixel_at(&out, 4, 0, 0, 3), vec![0, 255, 0]);
        assert_eq!(pixel_at(&out, 4, 3, 3, 3), vec![0, 255, 0]);
        assert_eq!(pixel_at(&out, 4, 1, 1, 3), vec![255, 0, 0]);
        assert_eq!(pixel_at(&out, 4, 2, 2, 3), vec![255, 0, 0]);
    }

    #[test]
    fn compose_center_rgba_carries_alpha_in_pad_region() {
        let src: Vec<u8> = vec![255, 0, 0, 255].repeat(4);
        let out = compose_center(&src, 2, 2, 4, 4, &[0, 0, 0, 0], 4);
        assert_eq!(out.len(), 4 * 4 * 4);
        assert_eq!(pixel_at(&out, 4, 0, 0, 4), vec![0, 0, 0, 0]);
        assert_eq!(pixel_at(&out, 4, 3, 0, 4), vec![0, 0, 0, 0]);
        assert_eq!(pixel_at(&out, 4, 1, 1, 4), vec![255, 0, 0, 255]);
        assert_eq!(pixel_at(&out, 4, 2, 2, 4), vec![255, 0, 0, 255]);
    }

    #[test]
    fn compose_center_identity_when_dims_match() {
        let src = make_test_image(3, 2);
        let out = compose_center(&src, 3, 2, 3, 2, &[0, 0, 0], 3);
        assert_eq!(out, src);
    }

    #[test]
    fn compose_center_odd_offsets_use_floor_division_rgb() {
        let src = vec![200, 100, 50];
        let out = compose_center(&src, 1, 1, 4, 1, &[10, 20, 30], 3);
        assert_eq!(pixel_at(&out, 4, 0, 0, 3), vec![10, 20, 30]);
        assert_eq!(pixel_at(&out, 4, 1, 0, 3), vec![200, 100, 50]);
        assert_eq!(pixel_at(&out, 4, 2, 0, 3), vec![10, 20, 30]);
        assert_eq!(pixel_at(&out, 4, 3, 0, 3), vec![10, 20, 30]);
    }

    #[test]
    fn compose_center_crops_when_src_wider_than_dst() {
        // 6x1 source (values 1..=6), target 2x1 → center-cropped to pixels 3,4.
        let src: Vec<u8> = vec![1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 5, 6, 6, 6];
        let out = compose_center(&src, 6, 1, 2, 1, &[0, 0, 0], 3);
        assert_eq!(out, vec![3, 3, 3, 4, 4, 4]);
    }

    #[test]
    fn compose_center_crops_when_src_taller_than_dst() {
        // 1x4 source, target 1x2 → keeps rows 1,2.
        let src: Vec<u8> = vec![1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4];
        let out = compose_center(&src, 1, 4, 1, 2, &[0, 0, 0], 3);
        assert_eq!(out, vec![2, 2, 2, 3, 3, 3]);
    }

    #[test]
    fn compose_center_crops_one_axis_pads_the_other() {
        // 4x1 source, target 2x3 → crop to 2x1 in x, pad vertically above and below.
        let src: Vec<u8> = vec![10, 10, 10, 20, 20, 20, 30, 30, 30, 40, 40, 40];
        let out = compose_center(&src, 4, 1, 2, 3, &[0, 0, 0], 3);
        // Row 0: pad. Row 1: src cols 1,2 = (20,30). Row 2: pad.
        assert_eq!(pixel_at(&out, 2, 0, 0, 3), vec![0, 0, 0]);
        assert_eq!(pixel_at(&out, 2, 1, 0, 3), vec![0, 0, 0]);
        assert_eq!(pixel_at(&out, 2, 0, 1, 3), vec![20, 20, 20]);
        assert_eq!(pixel_at(&out, 2, 1, 1, 3), vec![30, 30, 30]);
        assert_eq!(pixel_at(&out, 2, 0, 2, 3), vec![0, 0, 0]);
        assert_eq!(pixel_at(&out, 2, 1, 2, 3), vec![0, 0, 0]);
    }
}
