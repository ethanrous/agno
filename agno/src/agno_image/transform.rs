use std::error::Error;

use image::{RgbImage, imageops};
use tracing::debug;

use crate::{
    agno_image::AgnoImage,
    exif::{ExifContext, ExifValue, spec::ORIENTATION},
    sony_decoder::{DecodeError, Dimensions},
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

    // Try GPU resize first if available
    #[cfg(feature = "gpu")]
    if let Some(resized) = crate::resize_gpu::resize_gpu(
        a_img.as_slice(),
        a_img.width as u32,
        a_img.height as u32,
        new_width,
        new_height,
    ) {
        debug!("GPU resize complete");
        let exif = a_img.exif.clone();
        AgnoImage::free(&a_img);
        return Ok(AgnoImage::new(
            resized,
            new_width as u64,
            new_height as u64,
            exif,
        ));
    }

    // CPU fallback
    debug!("Using CPU resize");
    let rgb = RgbImage::from_raw(
        a_img.width as u32,
        a_img.height as u32,
        a_img.as_slice().to_vec(),
    )
    .ok_or(DecodeError::CorruptData(
        "Failed to create image from RGB data",
    ))?;

    let resized_img = image::imageops::resize(
        &rgb,
        new_width,
        new_height,
        image::imageops::FilterType::Lanczos3,
    );

    let exif = a_img.exif.clone();

    AgnoImage::free(&a_img);

    Ok(AgnoImage::new(
        resized_img.into_raw(),
        new_width as u64,
        new_height as u64,
        exif,
    ))
}

pub fn auto_rotate_image(
    ctx: &mut ExifContext,
    rgb: &[u8],
    dims: &mut Dimensions,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let orientation = match ctx.get_tag_value(ORIENTATION) {
        Some(ExifValue::Short(v)) if !v.is_empty() => v[0] as u8,
        _ => 1, // Default to normal orientation
    };

    // Create image from raw RGB data
    let img = RgbImage::from_raw(
        dims.output_width as u32,
        dims.output_height as u32,
        rgb.to_vec(),
    )
    .ok_or(DecodeError::CorruptData(
        "Failed to create image from RGB data",
    ))?;

    let result = match orientation {
        1 => img, // Normal - no transform
        2 => imageops::flip_horizontal(&img),
        3 => imageops::rotate180(&img),
        4 => imageops::flip_vertical(&img),
        5 => {
            // Transpose: rotate 90 CCW then flip horizontal
            let rotated = imageops::rotate270(&img);
            imageops::flip_horizontal(&rotated)
        }
        6 => {
            // Rotate 90 CW
            imageops::rotate90(&img)
        }
        7 => {
            // Transverse: rotate 90 CW then flip horizontal
            let rotated = imageops::rotate90(&img);
            imageops::flip_horizontal(&rotated)
        }
        8 => {
            // Rotate 90 CCW (270 CW)
            imageops::rotate270(&img)
        }
        _ => img, // Unknown - no transform
    };

    // Update dimensions if rotated 90 or 270 degrees
    if matches!(orientation, 5 | 6 | 7 | 8) {
        std::mem::swap(&mut dims.output_width, &mut dims.output_height);
    }

    Ok(result.into_raw())
}
