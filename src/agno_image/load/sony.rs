use std::{
    error::Error,
    fs::File,
    io::{Cursor, Seek, SeekFrom},
};

use crate::{
    agno_image::{AgnoImage, auto_rotate_image},
    demosaic::{BayerPattern, demosaic_bilinear_to_rgb8},
    exif::{
        ExifContext, ExifValue,
        spec::{BLACK_LEVEL, WB_RGGBLEVELS},
    },
    sony_decoder::{self, DecodeError, Dimensions},
    tiff::{SonyVariant, TiffDetectResult},
};

pub fn load_sony_raw(
    det: TiffDetectResult,
    mut file: &mut File,
    exif: ExifContext,
) -> Result<AgnoImage, Box<dyn Error>> {
    let mut dims = Dimensions {
        raw_width: det.raw.width as usize,
        raw_height: det.raw.height as usize,
        output_width: det.raw.width as usize,
        output_height: det.raw.height as usize,
    };

    // Read strips into memory once. Most ARW are single-strip; this works for multi-strip too.
    let buf = sony_decoder::read_concatenated_strips(
        &mut file,
        &det.raw.strip_offsets,
        &det.raw.strip_byte_counts,
    )?;

    let mut cursor = Cursor::new(buf);

    file.seek(SeekFrom::Start(0))?;
    let mut ctx = ExifContext::from_reader_auto(&mut file)?;

    // Auto-select decoder based on detection
    let decoded = match det.variant {
        SonyVariant::Arw2Compressed => {
            // ARW2: compressed row length equals pixel width; decoder expects row_len == active_width
            match sony_decoder::sony_arw2_load_raw(&mut cursor, dims) {
                Ok(result) => result,
                Err(e) => return Err(Box::new(e)),
            }
        }
        SonyVariant::ArwLjpeg => {
            // Legacy ARW: LibRaw adds 8 rows (raw_height += 8)
            dims.raw_height += 8;
            // zero_after_ff is usually true for JPEG-like streams
            let zero_after_ff = true;
            // Pass DNG version if present to match ljpeg_diff behavior; for native ARW, None is fine
            let dng_version = det.raw.dng_version;
            match sony_decoder::sony_arw_load_raw_from_stream(
                &mut cursor,
                dims,
                zero_after_ff,
                dng_version,
            ) {
                Ok(result) => result,
                Err(e) => return Err(Box::new(e)),
            }
        }
        SonyVariant::Uncompressed14 => {
            // Simple 14-bit uncompressed packed in 16-bit little-endian words
            match sony_decoder::sony_uncompressed14_load_raw(&mut cursor, dims) {
                Ok(result) => result,
                Err(e) => return Err(Box::new(e)),
            }
        }
        SonyVariant::Unknown => {
            return Err(Box::new(DecodeError::UnsupportedFormat(det.variant)));
        }
    };

    // Write a JPEG (choose your CFA pattern and black level)
    let pattern = BayerPattern::RGGB; // Adjust per camera/margins

    let black_level = match ctx.get_tag_value(BLACK_LEVEL) {
        Some(ExifValue::Short(v)) if !v.is_empty() => v[0],
        _ => 512,
    };

    let wb_raw = match ctx.get_tag_value(WB_RGGBLEVELS) {
        Some(ExifValue::Short(v)) if v.len() >= 4 => v,
        _ => &vec![1000, 1000, 1000, 1000],
    };

    let gamma = 2.2;
    let wb: [f32; 3] = [
        wb_raw[0] as f32 / 1000.0,
        wb_raw[1] as f32 / 1000.0,
        wb_raw[3] as f32 / 1000.0,
    ];

    let rgb = demosaic_bilinear_to_rgb8(
        &decoded.pixels,
        dims,
        pattern,
        black_level,
        decoded.white_level,
        wb, // wb_gains (EXIF if present; None falls back to gray-world)
        gamma,
    );

    // Auto-rotate based on EXIF Orientation tag
    let img = auto_rotate_image(&mut ctx, &rgb, &mut dims)?;

    Ok(AgnoImage::new(
        img,
        dims.output_width as u64,
        dims.output_height as u64,
        exif,
    ))
}
