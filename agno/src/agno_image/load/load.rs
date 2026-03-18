use std::{
    error::Error,
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom},
};

use crate::{
    agno_image::{
        AgnoImage,
        load::{load_canon_raw, load_heic, load_mov_thumbnail, load_pdf, load_sony_raw},
    },
    exif::ExifContext,
    tiff::{detect_raw, RawMaker, TiffDetectResult},
};

pub enum ImageType {
    Jpeg,
    Png,
    Webp,
    Pdf,
    Heic,
    QuickTimeMov,
    Mp4,
    SonyRaw(TiffDetectResult),
    CanonRaw(TiffDetectResult),
}

pub fn detect_image_type(reader: &mut File) -> Result<ImageType, Box<dyn Error>> {
    let mut buf = [0u8; 12];
    reader.seek(SeekFrom::Start(0))?;
    reader.read_exact(&mut buf)?;

    match [buf[0], buf[1]] {
        [0xFF, 0xD8] => Ok(ImageType::Jpeg),
        [0x89, b'P'] => Ok(ImageType::Png),
        [b'R', b'I'] => Ok(ImageType::Webp),
        // [0x25, 0x50, 0x44, 0x46]
        [0x25, 0x50] => Ok(ImageType::Pdf),
        [b'I', b'I'] | [b'M', b'M'] => {
            let det = detect_raw(reader)?;
            match det.maker {
                RawMaker::Canon(_) => Ok(ImageType::CanonRaw(det)),
                RawMaker::Sony(_) => Ok(ImageType::SonyRaw(det)),
            }
        }
        _ => {
            if &buf[4..8] == b"ftyp" {
                match &buf[8..12] {
                    b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" => Ok(ImageType::Heic),
                    b"qt  " => Ok(ImageType::QuickTimeMov),
                    // All other ISOBMFF are treated as MP4-family
                    _ => Ok(ImageType::Mp4),
                }
            } else if &buf[4..8] == b"wide"
                || &buf[4..8] == b"mdat"
                || &buf[4..8] == b"moov"
                || &buf[4..8] == b"free"
                || &buf[4..8] == b"skip"
            {
                // Classic QuickTime MOV without ftyp box
                Ok(ImageType::QuickTimeMov)
            } else {
                Err("Unsupported image format".into())
            }
        }
    }
}

pub fn load_agno_image_from_file(path: &str) -> Result<AgnoImage, Box<dyn Error>> {
    let mut file = File::open(path)?;

    // Try to load EXIF, but don't fail if it's missing (e.g., some JPEGs/PNGs)
    let exif = match ExifContext::from_reader_auto(&mut file) {
        Ok(ctx) => ctx,
        Err(e) => {
            tracing::warn!("Failed to extract EXIF from {path}: {e}");
            ExifContext::default()
        }
    };

    let mut img = match detect_image_type(&mut file)? {
        ImageType::Jpeg | ImageType::Png | ImageType::Webp => {
            let decoded = image::ImageReader::new(Cursor::new(std::fs::read(path)?))
                .with_guessed_format()?
                .decode()?
                .to_rgb8();
            let (width, height) = decoded.dimensions();
            Ok(AgnoImage::new(
                decoded.into_raw(),
                width as u64,
                height as u64,
                exif,
            ))
        }
        ImageType::Pdf => {
            if cfg!(feature = "pdf") {
                load_pdf(path, exif)
            } else {
                Err("PDF support is not enabled. Please enable the 'pdf' feature.".into())
            }
        }
        ImageType::SonyRaw(det) => load_sony_raw(det, &mut file, exif),
        ImageType::CanonRaw(det) => load_canon_raw(det, &mut file, exif),
        ImageType::Heic => load_heic(path, exif),
        ImageType::QuickTimeMov | ImageType::Mp4 => load_mov_thumbnail(&mut file, exif),
    }?;

    img.auto_rotate()?;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_jpeg_applies_orientation() {
        // sideways.jpeg: physical 4032x3024, EXIF orientation=6 (rotate 90 CW)
        // After rotation, width should be the smaller dimension
        let img = load_agno_image_from_file("../tests/data/sideways.jpeg").unwrap();
        assert_eq!(img.width, 3024, "Width should be 3024 after rotation");
        assert_eq!(img.height, 4032, "Height should be 4032 after rotation");
    }

    #[test]
    fn load_heic_applies_orientation() {
        // sideways2.heic: physical 4032x3024, EXIF orientation=6 (rotate 90 CW)
        let img = load_agno_image_from_file("../tests/data/sideways2.heic").unwrap();
        assert_eq!(img.width, 3024, "Width should be 3024 after rotation");
        assert_eq!(img.height, 4032, "Height should be 4032 after rotation");
    }

    #[test]
    fn load_heic3_applies_orientation() {
        // sideways3.heic: physical 4032x3024, EXIF orientation=6 (rotate 90 CW)
        let img = load_agno_image_from_file("../tests/data/sideways3.heic").unwrap();
        assert_eq!(img.width, 3024, "Width should be 3024 after rotation");
        assert_eq!(img.height, 4032, "Height should be 4032 after rotation");
    }
}
