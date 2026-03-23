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
    let resized = ops::resize_lanczos3(
        a_img.as_slice(),
        a_img.width as usize,
        a_img.height as usize,
        new_width as usize,
        new_height as usize,
    );
    let exif = a_img.exif.clone();
    AgnoImage::free(&a_img);
    Ok(AgnoImage::new(
        resized,
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
        _ => 1,
    };

    let w = dims.output_width;
    let h = dims.output_height;

    let result = match orientation {
        1 => rgb.to_vec(),
        2 => ops::flip_horizontal(rgb, w, h),
        3 => ops::rotate180(rgb, w, h),
        4 => ops::flip_vertical(rgb, w, h),
        5 => {
            let (rotated, _, _) = ops::rotate270(rgb, w, h);
            ops::flip_horizontal(&rotated, h, w)
        }
        6 => {
            let (rotated, _, _) = ops::rotate90(rgb, w, h);
            rotated
        }
        7 => {
            let (rotated, _, _) = ops::rotate90(rgb, w, h);
            ops::flip_horizontal(&rotated, h, w)
        }
        8 => {
            let (rotated, _, _) = ops::rotate270(rgb, w, h);
            rotated
        }
        _ => rgb.to_vec(),
    };

    if matches!(orientation, 5 | 6 | 7 | 8) {
        std::mem::swap(&mut dims.output_width, &mut dims.output_height);
    }

    Ok(result)
}
