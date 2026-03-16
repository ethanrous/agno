use std::{
    error::Error,
    fs::File,
    io::{Cursor, Seek, SeekFrom},
};

use tracing::{debug, trace};

use crate::{
    agno_image::AgnoImage,
    demosaic::{BayerPattern, demosaic_bilinear_to_rgb8},
    exif::{
        ExifContext, ExifValue,
        spec::{
            BLACK_LEVEL, COLOR_MATRIX1, COLOR_MATRIX2, COLOR_MATRIX3, SR2_COLOR_MATRIX,
            WB_RGGBLEVELS,
        },
    },
    sony_decoder::{self, DecodeError, Dimensions},
    tiff::{RawMaker, SonyVariant, TiffDetectResult},
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
    let ctx = ExifContext::from_reader_auto(&mut file)?;

    // Extract Sony variant from maker
    let variant = match det.maker {
        RawMaker::Sony(v) => v,
        _ => return Err("Not a Sony RAW file".into()),
    };

    // Auto-select decoder based on detection
    let decoded = match variant {
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
            return Err(Box::new(DecodeError::UnsupportedFormat(variant)));
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
    // WB_RGGBLEVELS format is [R, Gr, Gb, B] - normalize by green average so G=1.0
    let g_avg = (wb_raw[1] as f32 + wb_raw[2] as f32) / 2.0;
    let wb: [f32; 3] = [
        wb_raw[0] as f32 / g_avg, // R relative to G
        1.0,                      // G normalized to 1.0
        wb_raw[3] as f32 / g_avg, // B relative to G (index 3, not 2!)
    ];

    // Extract color correction matrix from EXIF (3x3 matrix stored as 9 SRationals)
    // This corrects for the camera sensor's color response
    // Try Sony-specific SR2 color matrix (0x7800) first, then fall back to DNG tags
    let color_matrix = ctx
        .get_tag_value(SR2_COLOR_MATRIX)
        .or_else(|| ctx.get_tag_value(COLOR_MATRIX3))
        .or_else(|| ctx.get_tag_value(COLOR_MATRIX1))
        .or_else(|| ctx.get_tag_value(COLOR_MATRIX2))
        .and_then(|v| match v {
            ExifValue::SRational(vals) if vals.len() >= 9 => {
                trace!(values = vals.len(), "Found color matrix as SRational");
                Some([
                    vals[0].0 as f32 / vals[0].1 as f32,
                    vals[1].0 as f32 / vals[1].1 as f32,
                    vals[2].0 as f32 / vals[2].1 as f32,
                    vals[3].0 as f32 / vals[3].1 as f32,
                    vals[4].0 as f32 / vals[4].1 as f32,
                    vals[5].0 as f32 / vals[5].1 as f32,
                    vals[6].0 as f32 / vals[6].1 as f32,
                    vals[7].0 as f32 / vals[7].1 as f32,
                    vals[8].0 as f32 / vals[8].1 as f32,
                ])
            }
            ExifValue::Short(vals) if vals.len() >= 9 => {
                // Sony SR2ColorMatrix: 9 signed 16-bit integers scaled by 1024
                trace!(values = vals.len(), "Found color matrix as Short");
                Some([
                    vals[0] as i16 as f32 / 1024.0,
                    vals[1] as i16 as f32 / 1024.0,
                    vals[2] as i16 as f32 / 1024.0,
                    vals[3] as i16 as f32 / 1024.0,
                    vals[4] as i16 as f32 / 1024.0,
                    vals[5] as i16 as f32 / 1024.0,
                    vals[6] as i16 as f32 / 1024.0,
                    vals[7] as i16 as f32 / 1024.0,
                    vals[8] as i16 as f32 / 1024.0,
                ])
            }
            _ => None,
        })
        .unwrap_or_else(|| {
            debug!("No color matrix found in EXIF, using identity matrix");
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
        });

    let rgb = demosaic_bilinear_to_rgb8(
        &decoded.pixels,
        dims,
        pattern,
        black_level,
        decoded.white_level,
        wb,
        color_matrix,
        gamma,
    );

    Ok(AgnoImage::new(
        rgb,
        dims.output_width as u64,
        dims.output_height as u64,
        exif,
    ))
}
