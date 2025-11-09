use std::error::Error;

use image::{RgbImage, imageops};
use log::debug;

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
        "Scaling image from {}x{} to {}x{}",
        a_img.width, a_img.height, new_width, new_height
    );

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
    let rot = ctx.get_tag_value(ORIENTATION);
    let img = match rot {
        Some(ExifValue::Short(v)) if !v.is_empty() => {
            let exif_orient = v[0] as u8;
            match exif_orient {
                1 | 2 | 3 | 4 | 5 | 7 => rgb, // No rotation
                6 => {
                    // 90-degree rotation
                    (dims.output_width, dims.output_height) =
                        (dims.output_height, dims.output_width);

                    rgb
                }
                8 => {
                    let img = RgbImage::from_raw(
                        dims.output_width as u32,
                        dims.raw_height as u32,
                        rgb.to_vec(),
                    )
                    .ok_or(DecodeError::CorruptData(
                        "Failed to create image from RGB data",
                    ))
                    .unwrap();

                    // 270-degree rotation
                    (dims.output_width, dims.output_height) =
                        (dims.output_height, dims.output_width);

                    &imageops::rotate270(&img)
                }
                _ => rgb,
            }
        }
        _ => rgb,
    };

    Ok(img.to_vec())
}
