use std::error::Error;

use libheif_rs::{ColorSpace, DecodingOptions, HeifContext, LibHeif, RgbChroma};

use crate::{agno_image::AgnoImage, exif::ExifContext};

pub fn load_heic(path: &str, exif: ExifContext) -> Result<AgnoImage, Box<dyn Error>> {
    let ctx = HeifContext::read_from_file(path)?;
    let handle = ctx.primary_image_handle()?;

    // Disable container transforms (irot/imir) so libheif returns raw
    // untransformed pixels. Rotation is handled centrally by auto_rotate
    // via the EXIF orientation tag, same as every other format.
    let mut decode_options = DecodingOptions::new()
        .ok_or("Failed to create HEIC decoding options")?;
    decode_options.set_ignore_transformations(true);

    let lib_heif = LibHeif::new();
    let image = lib_heif.decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), Some(decode_options))?;

    let planes = image.planes();
    let plane = planes
        .interleaved
        .ok_or("No interleaved RGB plane in HEIC")?;

    // Use ispe (coded) dimensions since container transforms are disabled.
    // handle.width()/height() returns post-transform dims regardless.
    let width = handle.ispe_width() as u64;
    let height = handle.ispe_height() as u64;
    let stride = plane.stride;

    // Handle stride padding (stride may be > width*3)
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for row in 0..height as usize {
        let start = row * stride;
        let end = start + width as usize * 3;
        rgb.extend_from_slice(&plane.data[start..end]);
    }

    Ok(AgnoImage::new(rgb, width, height, exif))
}

#[cfg(test)]
mod tests {
    use crate::agno_image::load::{detect_image_type, load_agno_image_from_file};
    use crate::exif::spec;
    use std::fs::File;

    #[test]
    fn detect_heic_format() {
        let mut file = File::open("../tests/data/test-heic.heic").unwrap();
        let image_type = detect_image_type(&mut file).unwrap();
        assert!(matches!(image_type, crate::agno_image::load::ImageType::Heic));
    }

    #[test]
    fn detect_jpeg_still_works() {
        let test_files = [
            ("../tests/data/sony.ARW", "SonyRaw"),
            ("../tests/data/cannon.CR2", "CanonRaw"),
        ];
        for (path, expected) in &test_files {
            let mut file = File::open(path).unwrap();
            let image_type = detect_image_type(&mut file).unwrap();
            match expected {
                &"SonyRaw" => assert!(
                    matches!(image_type, crate::agno_image::load::ImageType::SonyRaw(_)),
                    "Expected SonyRaw for {path}"
                ),
                &"CanonRaw" => assert!(
                    matches!(image_type, crate::agno_image::load::ImageType::CanonRaw(_)),
                    "Expected CanonRaw for {path}"
                ),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn load_heic_image() {
        let img = load_agno_image_from_file("../tests/data/test-heic.heic").unwrap();
        assert!(img.width > 0, "Width should be non-zero");
        assert!(img.height > 0, "Height should be non-zero");
        assert_eq!(
            img.as_slice().len(),
            (img.width * img.height * 3) as usize,
            "Pixel data length should be width * height * 3 (RGB8)"
        );
    }

    #[test]
    fn load_heic_has_exif() {
        let img = load_agno_image_from_file("../tests/data/test-heic.heic").unwrap();
        assert!(img.exif.get_tag_value(spec::MAKE).is_some());
        assert!(img.exif.get_tag_value(spec::OFFSET_TIME).is_some(),
            "Must follow ExifIFD sub-IFD to find OffsetTime");
        assert!(img.exif.get_tag_value(spec::GPS_LATITUDE_REF).is_some(),
            "Must follow GPS sub-IFD to find GPSLatitudeRef");
    }
}
