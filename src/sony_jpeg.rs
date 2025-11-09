use image::{RgbImage, imageops};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use tiff::encoder::*;

use crate::demosaic::{BayerPattern, demosaic_bilinear_to_rgb8};
use crate::sony_decoder::{DecodeError, Dimensions, SonyLoadResult};

pub fn write_tiff_from_sony_result_to_path<P: AsRef<Path>>(
    result: &mut SonyLoadResult,
    dims: Dimensions,
    pattern: BayerPattern,
    black_level: u16,
    wb_gains: Option<[f32; 3]>,
    gamma: f32,
    quality: u8,
    out_path: P,
) -> Result<(), DecodeError> {
    let rgb = demosaic_bilinear_to_rgb8(
        &mut result.pixels,
        dims,
        pattern,
        black_level,
        result.white_level,
        wb_gains.unwrap_or([1.0, 1.0, 1.0]),
        gamma,
    );

    let mut file = File::create(out_path).map_err(DecodeError::Io)?;

    let mut tiff = TiffEncoder::new(&mut file).unwrap();
    tiff.write_image::<colortype::RGB8>(dims.output_width as u32, dims.output_height as u32, &rgb)
        .unwrap();

    write_tiff_from_rgb8_writer(
        &mut file,
        &rgb,
        dims.output_width as u32,
        dims.output_height as u32,
        quality,
    )?;

    Ok(())
}

pub fn write_webp_from_rgb8_writer<W: Write>(
    writer: &mut W,
    rgb: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<(), DecodeError> {
    let enc = webp::Encoder::new(rgb, webp::PixelLayout::Rgb, width, height);
    let encoded = enc.encode(quality as f32);
    writer.write_all(&encoded)?;

    Ok(())
}

fn write_tiff_from_rgb8_writer<W: Write>(
    writer: &mut W,
    rgb: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<(), DecodeError> {
    let img = RgbImage::from_raw(width, height, rgb.to_vec()).ok_or(DecodeError::CorruptData(
        "Failed to create image from RGB data",
    ))?;

    let rotated_img = imageops::rotate270(&img);

    let enc = webp::Encoder::new(rotated_img.as_raw(), webp::PixelLayout::Rgb, height, width);

    let encoded = enc.encode(quality as f32);

    writer.write_all(&encoded)?;

    Ok(())
}
