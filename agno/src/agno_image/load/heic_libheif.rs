//! HEIC loader using the libheif C library (via libheif-rs).
//!
//! Fallback path enabled by the `heic-c` feature flag. Uses system-installed
//! libheif for full HEIC decoding (container parsing + HEVC), bypassing the
//! native HEVC decoder in `codec/hevc/`.

use std::error::Error;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use libheif_rs::{ColorSpace, DecodingOptions, HeifContext, LibHeif, RgbChroma};

use crate::agno_image::AgnoImage;
use crate::exif::ExifContext;

/// Load a HEIC image using the libheif C library.
///
/// Reads the entire file into memory, passes it to libheif for decoding,
/// and converts the result to an AgnoImage with RGB8 pixel data.
pub fn load_heic_libheif(file: &mut File, exif: ExifContext) -> Result<AgnoImage, Box<dyn Error>> {
    file.seek(SeekFrom::Start(0))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)?;

    let lib_heif = LibHeif::new();
    let ctx = HeifContext::read_from_bytes(&data)?;
    let handle = ctx.primary_image_handle()?;

    // Use ISPE (original) dimensions since we skip ISOBMFF transforms.
    let width = handle.ispe_width() as u64;
    let height = handle.ispe_height() as u64;

    // Skip ISOBMFF transforms (irot/imir) so our pipeline handles EXIF
    // orientation consistently via auto_rotate in load_agno_image_from_file.
    let mut options = DecodingOptions::new()
        .ok_or("Failed to allocate libheif DecodingOptions")?;
    options.set_ignore_transformations(true);

    let image = lib_heif.decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), Some(options))?;
    let planes = image.planes();
    let interleaved = planes
        .interleaved
        .ok_or("libheif decode produced no interleaved RGB plane")?;

    // libheif may use a stride larger than width*3 (row padding).
    // Copy row-by-row to produce a packed RGB8 buffer.
    let stride = interleaved.stride;
    let row_bytes = (width as usize) * 3;
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);

    for y in 0..height as usize {
        let row_start = y * stride;
        let row_end = row_start + row_bytes;
        if row_end <= interleaved.data.len() {
            rgb.extend_from_slice(&interleaved.data[row_start..row_end]);
        } else {
            rgb.resize(rgb.len() + row_bytes, 0);
        }
    }

    Ok(AgnoImage::new(rgb, width, height, exif))
}
