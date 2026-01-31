use std::{
    error::Error,
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
};

use crate::{
    agno_image::{
        AgnoImage,
        load::{load_canon_raw, load_pdf, load_sony_raw},
    },
    exif::ExifContext,
    tiff::{detect_raw, RawMaker, TiffDetectResult},
};

pub enum ImageType {
    Jpeg,
    Png,
    Webp,
    Pdf,
    SonyRaw(TiffDetectResult),
    CanonRaw(TiffDetectResult),
}

pub fn detect_image_type(reader: &mut File) -> Result<ImageType, Box<dyn Error>> {
    let mut buf = [0u8; 2];
    reader.seek(SeekFrom::Start(0))?;
    reader.read_exact(&mut buf)?;

    match buf {
        [0xFF, 0xD8] => Ok(ImageType::Jpeg),
        [0x89, b'P'] => Ok(ImageType::Png),
        [b'R', b'I'] => Ok(ImageType::Webp),
        // [0x25, 0x50, 0x44, 0x46]
        [0x25, 0x50] => Ok(ImageType::Pdf),
        [b'I', b'I'] | [b'M', b'M'] => {
            let det = detect_raw(reader)?;
            // Distinguish Sony from Canon based on maker
            match det.maker {
                RawMaker::Canon(_) => Ok(ImageType::CanonRaw(det)),
                RawMaker::Sony(_) => Ok(ImageType::SonyRaw(det)),
            }
        }
        _ => Err("Unsupported image format".into()),
    }
}

pub fn load_agno_image_from_file(path: &str) -> Result<AgnoImage, Box<dyn Error>> {
    let mut file = File::open(path)?;

    // Try to load EXIF, but don't fail if it's missing (e.g., some JPEGs/PNGs)
    let exif = ExifContext::from_reader_auto(&mut file).unwrap_or_default();

    match detect_image_type(&mut file)? {
        ImageType::Jpeg | ImageType::Png | ImageType::Webp => {
            // For JPEG, use image crate directly
            let img = image::ImageReader::new(Cursor::new(std::fs::read(path)?))
                .with_guessed_format()?
                .decode()?
                .to_rgb8();

            let (width, height) = img.dimensions();
            return Ok(AgnoImage::new(
                img.into_raw(),
                width as u64,
                height as u64,
                exif,
            ));
        }
        ImageType::Pdf => {
            if cfg!(feature = "pdf") {
                load_pdf(path, exif)
            } else {
                Err("PDF support is not enabled. Please enable the 'pdf' feature.".into())
            }
        }
        ImageType::SonyRaw(det) => {
            // For Sony RAW, proceed with ARW decoding
            load_sony_raw(det, &mut file, exif)
        }
        ImageType::CanonRaw(det) => {
            // For Canon RAW, proceed with CR2 decoding
            load_canon_raw(det, &mut file, exif)
        }
    }
}
