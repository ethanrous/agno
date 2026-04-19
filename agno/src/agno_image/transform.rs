use std::error::Error;

use tracing::debug;

use super::ops;
use crate::{
    agno_image::AgnoImage,
    exif::{ExifContext, ExifValue, spec::ORIENTATION},
    sony_decoder::Dimensions,
};

/// Largest `(w, h)` that preserves source aspect ratio and fits inside `(dst_w, dst_h)`.
/// Clamped to ≥1 pixel per axis; never exceeds the target.
pub(crate) fn compute_fit_dims(
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> (u32, u32) {
    if src_w == 0 || src_h == 0 {
        return (dst_w.max(1), dst_h.max(1));
    }
    let scale = (dst_w as f64 / src_w as f64).min(dst_h as f64 / src_h as f64);
    let inner_w = ((src_w as f64 * scale).round() as u32).max(1).min(dst_w);
    let inner_h = ((src_h as f64 * scale).round() as u32).max(1).min(dst_h);
    (inner_w, inner_h)
}

pub fn scale_image(
    a_img: AgnoImage,
    new_width: u32,
    new_height: u32,
) -> Result<AgnoImage, Box<dyn Error>> {
    debug!(
        from_width = a_img.width,
        from_height = a_img.height,
        to_width = new_width,
        to_height = new_height,
        "Scaling image"
    );

    let ch = a_img.channels;

    // Try GPU resize first if available
    #[cfg(feature = "gpu")]
    if let Some(resized) = crate::resize_gpu::resize_gpu(
        a_img.as_slice(),
        a_img.width as u32,
        a_img.height as u32,
        new_width,
        new_height,
        a_img.channels as u32,
    ) {
        debug!("GPU resize complete");
        let exif = a_img.exif.clone();
        AgnoImage::free(&a_img);
        return Ok(AgnoImage::new_with_channels(
            resized,
            new_width as u64,
            new_height as u64,
            ch,
            exif,
        ));
    }

    // CPU fallback
    debug!("Using CPU resize");
    let resized = ops::resize_lanczos3(
        a_img.as_slice(),
        a_img.width as usize,
        a_img.height as usize,
        new_width as usize,
        new_height as usize,
        ch as usize,
    );
    let exif = a_img.exif.clone();
    AgnoImage::free(&a_img);
    Ok(AgnoImage::new_with_channels(
        resized,
        new_width as u64,
        new_height as u64,
        ch,
        exif,
    ))
}

pub fn auto_rotate_image(
    ctx: &mut ExifContext,
    rgb: &[u8],
    dims: &mut Dimensions,
    channels: usize,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let orientation = match ctx.get_tag_value(ORIENTATION) {
        Some(ExifValue::Short(v)) if !v.is_empty() => v[0] as u8,
        _ => 1,
    };

    let w = dims.output_width;
    let h = dims.output_height;

    let result = match orientation {
        1 => rgb.to_vec(),
        2 => ops::flip_horizontal(rgb, w, h, channels),
        3 => ops::rotate180(rgb, w, h, channels),
        4 => ops::flip_vertical(rgb, w, h, channels),
        5 => {
            let (rotated, _, _) = ops::rotate270(rgb, w, h, channels);
            ops::flip_horizontal(&rotated, h, w, channels)
        }
        6 => {
            let (rotated, _, _) = ops::rotate90(rgb, w, h, channels);
            rotated
        }
        7 => {
            let (rotated, _, _) = ops::rotate90(rgb, w, h, channels);
            ops::flip_horizontal(&rotated, h, w, channels)
        }
        8 => {
            let (rotated, _, _) = ops::rotate270(rgb, w, h, channels);
            rotated
        }
        _ => rgb.to_vec(),
    };

    if matches!(orientation, 5..=8) {
        std::mem::swap(&mut dims.output_width, &mut dims.output_height);
    }

    Ok(result)
}

/// RGB or RGBA color for the `--no-stretch` pad area.
#[derive(Debug, Copy, Clone)]
pub enum PadColor {
    Rgb([u8; 3]),
    Rgba([u8; 4]),
}

impl PadColor {
    pub fn channels(&self) -> u8 {
        match self {
            PadColor::Rgb(_) => 3,
            PadColor::Rgba(_) => 4,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            PadColor::Rgb(b) => b,
            PadColor::Rgba(b) => b,
        }
    }
}

/// Promote an RGB image to RGBA by inserting `alpha = 255` after each RGB triple.
fn promote_rgb_to_rgba(a_img: AgnoImage) -> AgnoImage {
    let src = a_img.as_slice();
    let pixel_count = (a_img.width * a_img.height) as usize;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for i in 0..pixel_count {
        let off = i * 3;
        rgba.push(src[off]);
        rgba.push(src[off + 1]);
        rgba.push(src[off + 2]);
        rgba.push(255);
    }
    let width = a_img.width;
    let height = a_img.height;
    let exif = a_img.exif.clone();
    AgnoImage::free(&a_img);
    AgnoImage::new_with_channels(rgba, width, height, 4, exif)
}

/// Resize without stretching: scale to largest aspect-preserving fit within
/// `(new_width, new_height)`, then center inside a target-sized buffer filled
/// with `pad`. If `pad` carries alpha and the source is RGB, the source is
/// promoted to RGBA first so the output has real per-pixel transparency in
/// the pad region.
pub fn scale_image_no_stretch(
    mut a_img: AgnoImage,
    new_width: u32,
    new_height: u32,
    pad: PadColor,
) -> Result<AgnoImage, Box<dyn Error>> {
    if pad.channels() == 4 && a_img.channels == 3 {
        a_img = promote_rgb_to_rgba(a_img);
    }
    let output_channels = a_img.channels;
    let pad_bytes_4: [u8; 4] = match (pad, output_channels) {
        (PadColor::Rgb([r, g, b]), 4) => [r, g, b, 255],
        (PadColor::Rgba(b), _) => b,
        (PadColor::Rgb([r, g, b]), _) => [r, g, b, 0],
    };
    let pad_slice: &[u8] = match output_channels {
        3 => &pad_bytes_4[..3],
        _ => &pad_bytes_4[..4],
    };

    let (inner_w, inner_h) = compute_fit_dims(
        a_img.width as u32,
        a_img.height as u32,
        new_width,
        new_height,
    );

    debug!(
        from_width = a_img.width,
        from_height = a_img.height,
        to_width = new_width,
        to_height = new_height,
        inner_width = inner_w,
        inner_height = inner_h,
        channels = output_channels,
        "Scaling image without stretching"
    );

    if inner_w == new_width && inner_h == new_height {
        return scale_image(a_img, new_width, new_height);
    }

    let scaled = scale_image(a_img, inner_w, inner_h)?;
    let padded = ops::pad_center(
        scaled.as_slice(),
        inner_w as usize,
        inner_h as usize,
        new_width as usize,
        new_height as usize,
        pad_slice,
        output_channels as usize,
    );
    let exif = scaled.exif.clone();
    AgnoImage::free(&scaled);
    Ok(AgnoImage::new_with_channels(
        padded,
        new_width as u64,
        new_height as u64,
        output_channels,
        exif,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_dims_square_into_wider_box_pillarboxes() {
        let (iw, ih) = compute_fit_dims(512, 512, 1024, 512);
        assert_eq!((iw, ih), (512, 512));
    }

    #[test]
    fn fit_dims_wide_into_square_letterboxes() {
        let (iw, ih) = compute_fit_dims(1000, 500, 400, 400);
        assert_eq!((iw, ih), (400, 200));
    }

    #[test]
    fn fit_dims_identity_when_target_matches_source() {
        let (iw, ih) = compute_fit_dims(800, 600, 800, 600);
        assert_eq!((iw, ih), (800, 600));
    }

    #[test]
    fn fit_dims_upscale_uniformly() {
        let (iw, ih) = compute_fit_dims(512, 512, 2048, 2048);
        assert_eq!((iw, ih), (2048, 2048));
    }

    #[test]
    fn fit_dims_clamps_to_minimum_one_pixel() {
        let (iw, ih) = compute_fit_dims(1000, 1, 100, 100);
        assert_eq!(iw, 100);
        assert!(ih >= 1);
    }

    #[test]
    fn fit_dims_never_exceeds_target() {
        let (iw, ih) = compute_fit_dims(1001, 501, 400, 400);
        assert!(iw <= 400 && ih <= 400);
    }
}
