//! Canon CR2 RAW image loader

use std::{
    error::Error,
    fs::File,
    io::{Cursor, Seek, SeekFrom},
};

use tracing::debug;

use crate::{
    agno_image::{AgnoImage, auto_rotate_image},
    canon_decoder::{self, CanonLoadResult, Cr2SliceInfo},
    demosaic::{BayerPattern, demosaic_bilinear_to_rgb8},
    exif::{
        ExifContext, ExifValue,
        spec::{
            AS_SHOT_NEUTRAL, BLACK_LEVEL_DNG, COLOR_MATRIX1, COLOR_MATRIX2, WHITE_LEVEL,
        },
    },
    sony_decoder::{DecodeError, Dimensions, read_concatenated_strips},
    tiff::{CanonVariant, TiffDetectResult},
};

/// Load a Canon CR2 RAW image
pub fn load_canon_raw(
    det: TiffDetectResult,
    file: &mut File,
    exif: ExifContext,
) -> Result<AgnoImage, Box<dyn Error>> {
    let mut dims = Dimensions {
        raw_width: det.raw.width as usize,
        raw_height: det.raw.height as usize,
        output_width: det.raw.width as usize,
        output_height: det.raw.height as usize,
    };

    // Read strips into memory
    let buf = read_concatenated_strips(
        file,
        &det.raw.strip_offsets,
        &det.raw.strip_byte_counts,
    )?;

    let mut cursor = Cursor::new(buf);

    // Re-read EXIF for metadata extraction
    file.seek(SeekFrom::Start(0))?;
    let mut ctx = ExifContext::from_reader_auto(file)?;

    // Get Canon variant from detection result
    let variant = match det.maker {
        crate::tiff::RawMaker::Canon(v) => v,
        _ => return Err("Not a Canon RAW file".into()),
    };

    // Parse CR2 slice info for proper image reconstruction
    let slice_info = det.raw.cr2_slices.as_ref().map(|slices| {
        debug!(slices = ?slices, "CR2 raw slice tag");
        Cr2SliceInfo::from_raw(slices, dims.raw_width)
    });

    // Decode based on variant
    let decoded: CanonLoadResult = match variant {
        CanonVariant::LosslessJpeg => {
            canon_decoder::canon_ljpeg_load_raw(&mut cursor, dims, slice_info)?
        }
        CanonVariant::Uncompressed => {
            canon_decoder::canon_uncompressed_load_raw(&mut cursor, dims, det.raw.bits_per_sample)?
        }
        CanonVariant::Unknown => {
            return Err(Box::new(DecodeError::UnsupportedFormat(
                crate::tiff::SonyVariant::Unknown,
            )));
        }
    };

    // Canon uses RGGB Bayer pattern (RG/GB filter pattern)
    let pattern = BayerPattern::RGGB;

    // Extract black level from DNG tag (0xC61A) or use default
    // Canon typically has black level around 2048 for 14-bit
    let black_level = match ctx.get_tag_value(BLACK_LEVEL_DNG) {
        Some(ExifValue::Short(v)) if !v.is_empty() => v[0],
        Some(ExifValue::Long(v)) if !v.is_empty() => v[0] as u16,
        Some(ExifValue::Rational(v)) if !v.is_empty() => (v[0].0 / v[0].1.max(1)) as u16,
        _ => 2048, // Canon default black level
    };

    // Extract white level from DNG tag (0xC61D) or use decoded value
    let white_level = match ctx.get_tag_value(WHITE_LEVEL) {
        Some(ExifValue::Short(v)) if !v.is_empty() => v[0],
        Some(ExifValue::Long(v)) if !v.is_empty() => v[0] as u16,
        _ => decoded.white_level,
    };

    // Extract white balance from AsShotNeutral (0xC628)
    // Format is [R, G, B] as rationals - we need to invert and normalize
    let wb = extract_canon_white_balance(&ctx);

    debug!(
        r_gain = wb[0],
        g_gain = wb[1],
        b_gain = wb[2],
        "Canon WB gains"
    );

    // Extract color matrix from DNG tags
    let color_matrix = ctx
        .get_tag_value(COLOR_MATRIX1)
        .or_else(|| ctx.get_tag_value(COLOR_MATRIX2))
        .and_then(|v| match v {
            ExifValue::SRational(vals) if vals.len() >= 9 => {
                debug!(values = vals.len(), "Found color matrix");
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
            _ => None,
        })
        .unwrap_or_else(|| {
            debug!("No color matrix found, using identity matrix");
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
        });

    debug!(
        matrix = ?color_matrix,
        "Color matrix applied"
    );

    let gamma = 2.2;

    let rgb = demosaic_bilinear_to_rgb8(
        &decoded.pixels,
        dims,
        pattern,
        black_level,
        white_level,
        wb,
        color_matrix,
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

/// Extract white balance gains from Canon EXIF data
///
/// Canon CR2 files store white balance in maker notes, which are complex to parse.
/// If DNG AsShotNeutral is available, use that. Otherwise, use daylight white balance
/// as a reasonable default for Canon cameras.
fn extract_canon_white_balance(ctx: &ExifContext) -> [f32; 3] {
    // Try AsShotNeutral first (DNG format)
    if let Some(ExifValue::Rational(vals)) = ctx.get_tag_value(AS_SHOT_NEUTRAL) {
        if vals.len() >= 3 {
            // AsShotNeutral is [R, G, B] - these are the multipliers to achieve neutral
            // We need to invert them to get gains: gain = 1/neutral
            let r_neutral = vals[0].0 as f32 / vals[0].1.max(1) as f32;
            let g_neutral = vals[1].0 as f32 / vals[1].1.max(1) as f32;
            let b_neutral = vals[2].0 as f32 / vals[2].1.max(1) as f32;

            if r_neutral > 0.0 && g_neutral > 0.0 && b_neutral > 0.0 {
                // Invert and normalize by green
                let r_gain = (1.0 / r_neutral) / (1.0 / g_neutral);
                let b_gain = (1.0 / b_neutral) / (1.0 / g_neutral);
                return [r_gain, 1.0, b_gain];
            }
        }
    }

    // Canon CR2 default: daylight white balance
    // These values are typical for Canon cameras under daylight illumination
    // R needs higher gain (sensor is less sensitive to red), B needs moderate boost
    // Based on dcraw daylight multipliers: ~2.1, ~0.94, ~1.34 normalized to G=1
    [2.2, 1.0, 1.4]
}
