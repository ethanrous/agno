use std::error::Error;

use tracing::debug;

use super::ops;
use crate::{
    agno_image::AgnoImage,
    exif::{ExifContext, ExifValue, spec::ORIENTATION},
    sony_decoder::Dimensions,
};

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

/// Resize without stretching: preserve source pixel dimensions, center it
/// inside a `(new_width, new_height)` buffer, and either pad with `pad` where
/// the canvas is larger or center-crop where it is smaller. The source is
/// never scaled. If `pad` carries alpha and the source is RGB, the source is
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

    debug!(
        from_width = a_img.width,
        from_height = a_img.height,
        to_width = new_width,
        to_height = new_height,
        channels = output_channels,
        "Composing image into target canvas without stretching"
    );

    if a_img.width as u32 == new_width && a_img.height as u32 == new_height {
        return Ok(a_img);
    }

    let composed = ops::compose_center(
        a_img.as_slice(),
        a_img.width as usize,
        a_img.height as usize,
        new_width as usize,
        new_height as usize,
        pad_slice,
        output_channels as usize,
    );
    let exif = a_img.exif.clone();
    AgnoImage::free(&a_img);
    Ok(AgnoImage::new_with_channels(
        composed,
        new_width as u64,
        new_height as u64,
        output_channels,
        exif,
    ))
}

