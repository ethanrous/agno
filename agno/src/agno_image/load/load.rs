use std::{
    error::Error,
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use crate::{
    agno_image::{
        AgnoImage,
        load::{load_canon_raw, load_heic, load_mov_thumbnail, load_sony_raw},
    },
    exif::ExifContext,
    tiff::{RawMaker, TiffDetectResult, detect_raw},
};

pub enum ImageType {
    Jpeg,
    Png,
    Webp,
    Gif,
    Dicom,
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

    // DICOM Part-10 first: the 128-byte preamble may contain arbitrary bytes —
    // even another format's magic — so the authoritative signal is the "DICM"
    // marker at offset 128. Checking it before the leading-magic match prevents
    // a DICOM file with a non-zero preamble being misrouted to another decoder.
    let mut dicm = [0u8; 4];
    if reader.seek(SeekFrom::Start(128)).is_ok()
        && reader.read_exact(&mut dicm).is_ok()
        && &dicm == b"DICM"
    {
        return Ok(ImageType::Dicom);
    }
    // The DICOM probe moved the cursor; rewind to the start so detection stays
    // side-effect-free for non-DICOM inputs. The match below reads from `buf` and
    // detect_raw re-seeks, but callers shouldn't have to assume a probe position.
    reader.seek(SeekFrom::Start(0))?;

    match [buf[0], buf[1]] {
        [0xFF, 0xD8] => Ok(ImageType::Jpeg),
        [0x89, b'P'] => Ok(ImageType::Png),
        [b'R', b'I'] => Ok(ImageType::Webp),
        [b'G', b'I'] if buf[2] == b'F' => Ok(ImageType::Gif),
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
        #[cfg(feature = "jpeg")]
        ImageType::Jpeg => {
            let file_data = std::fs::read(path)?;
            let (rgb, width, height) = crate::codec::jpeg::decode_jpeg(&file_data)?;
            Ok(AgnoImage::new(rgb, width as u64, height as u64, exif))
        }
        #[cfg(not(feature = "jpeg"))]
        ImageType::Jpeg => {
            Err("JPEG support is not enabled. Please enable the 'jpeg' feature.".into())
        }
        #[cfg(feature = "png")]
        ImageType::Png => {
            let file_data = std::fs::read(path)?;
            let (rgb, width, height) = crate::codec::png::decode_png(&file_data)?;
            Ok(AgnoImage::new(rgb, width as u64, height as u64, exif))
        }
        #[cfg(not(feature = "png"))]
        ImageType::Png => {
            Err("PNG support is not enabled. Please enable the 'png' feature.".into())
        }
        #[cfg(feature = "webp")]
        ImageType::Webp => {
            let file_data = std::fs::read(path)?;
            let (rgb, width, height) = crate::codec::webp::decode_webp(&file_data)?;
            Ok(AgnoImage::new(rgb, width as u64, height as u64, exif))
        }
        #[cfg(not(feature = "webp"))]
        ImageType::Webp => {
            Err("WebP support is not enabled. Please enable the 'webp' feature.".into())
        }
        #[cfg(feature = "gif")]
        ImageType::Gif => {
            let file_data = std::fs::read(path)?;
            super::load_gif_frame_from_bytes(&file_data, 0, exif)
        }
        #[cfg(not(feature = "gif"))]
        ImageType::Gif => {
            Err("GIF support is not enabled. Please enable the 'gif' feature.".into())
        }
        #[cfg(feature = "dicom")]
        ImageType::Dicom => {
            let file_data = std::fs::read(path)?;
            super::load_dicom_from_bytes(&file_data, exif)
        }
        #[cfg(not(feature = "dicom"))]
        ImageType::Dicom => {
            Err("DICOM support is not enabled. Please enable the 'dicom' feature.".into())
        }
        #[cfg(feature = "pdf")]
        ImageType::Pdf => {
            let file_data = std::fs::read(path)?;
            super::load_pdf_page_from_bytes(&file_data, 0, None, exif)
        }
        #[cfg(not(feature = "pdf"))]
        ImageType::Pdf => {
            Err("PDF support is not enabled. Please enable the 'pdf' feature.".into())
        }
        ImageType::SonyRaw(det) => load_sony_raw(det, &mut file, exif),
        ImageType::CanonRaw(det) => load_canon_raw(det, &mut file, exif),
        ImageType::Heic => load_heic(&mut file, exif),
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

    // sideways3.heic test removed: file does not exist in the test data directory

    #[cfg(feature = "gif")]
    #[test]
    fn auto_load_gif_returns_frame_zero() {
        // Build a tiny GIF on disk and load it via the public entry point.
        use image::{Frame, RgbaImage, codecs::gif::GifEncoder};
        use std::io::Cursor;

        let mut rgba = RgbaImage::new(2, 1);
        rgba.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
        rgba.put_pixel(1, 0, image::Rgba([200, 100, 50, 255]));

        let mut bytes = Vec::new();
        {
            let mut enc = GifEncoder::new(Cursor::new(&mut bytes));
            enc.encode_frame(Frame::new(rgba.clone())).unwrap();
        }
        let dir = std::env::temp_dir();
        let unique_suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = dir.join(format!(
            "agno_test_load_{}_{}.gif",
            std::process::id(),
            unique_suffix
        ));
        std::fs::write(&path, &bytes).unwrap();

        let img = load_agno_image_from_file(path.to_str().unwrap()).unwrap();
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 1);
        assert_eq!(img.page_count, 1);
        assert_eq!(img.as_slice().len(), 2 * 1 * 3);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn detects_dicom_despite_format_magic_in_preamble() {
        // A DICOM whose 128-byte preamble happens to start with JPEG's FF D8 magic
        // must still be detected as DICOM via the DICM marker at offset 128.
        let mut bytes = vec![0u8; 132];
        bytes[0] = 0xFF;
        bytes[1] = 0xD8;
        bytes[128..132].copy_from_slice(b"DICM");

        let dir = std::env::temp_dir();
        let unique_suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = dir.join(format!(
            "agno_dicom_preamble_{}_{}.dcm",
            std::process::id(),
            unique_suffix
        ));
        std::fs::write(&path, &bytes).unwrap();

        let mut f = File::open(&path).unwrap();
        let detected = detect_image_type(&mut f).unwrap();
        assert!(
            matches!(detected, ImageType::Dicom),
            "expected Dicom, got something else"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn detect_image_type_rewinds_cursor_for_non_dicom() {
        // After probing offset 128 for the DICOM marker, detection must leave a
        // non-DICOM reader's cursor at the start so subsequent reads are predictable.
        let mut bytes = vec![0u8; 200];
        bytes[0] = 0xFF; // JPEG SOI
        bytes[1] = 0xD8;

        let dir = std::env::temp_dir();
        let unique_suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = dir.join(format!(
            "agno_detect_rewind_{}_{}.jpg",
            std::process::id(),
            unique_suffix
        ));
        std::fs::write(&path, &bytes).unwrap();

        let mut f = File::open(&path).unwrap();
        let detected = detect_image_type(&mut f).unwrap();
        assert!(matches!(detected, ImageType::Jpeg));
        assert_eq!(
            f.stream_position().unwrap(),
            0,
            "cursor must be rewound after a non-DICOM probe"
        );
        std::fs::remove_file(&path).ok();
    }
}
