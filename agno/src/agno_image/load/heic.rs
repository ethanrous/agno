use std::error::Error;
use std::fs::File;
use std::io::Seek;

use crate::agno_image::AgnoImage;
use crate::codec::heif::parse_heif;
use crate::codec::hevc::decode_hevc_still;
use crate::exif::ExifContext;

pub fn load_heic(file: &mut File, exif: ExifContext) -> Result<AgnoImage, Box<dyn Error>> {
    file.rewind()?;
    let heif = parse_heif(file)?;

    if heif.grid_cols == 1 && heif.grid_rows == 1 {
        // Single-tile image
        let picture = decode_hevc_still(&heif.hvcc, &heif.tiles[0])?;
        let rgb = picture.to_rgb8();
        Ok(AgnoImage::new(rgb, picture.width as u64, picture.height as u64, exif))
    } else {
        // Grid image: decode each tile and stitch
        let out_w = heif.width as usize;
        let out_h = heif.height as usize;
        let cols = heif.grid_cols as usize;
        let rows = heif.grid_rows as usize;

        // Decode all tiles
        let mut tile_rgbs: Vec<(Vec<u8>, u32, u32)> = Vec::with_capacity(heif.tiles.len());
        for tile_data in &heif.tiles {
            let pic = decode_hevc_still(&heif.hvcc, tile_data)?;
            let w = pic.width;
            let h = pic.height;
            tile_rgbs.push((pic.to_rgb8(), w, h));
        }

        // Stitch tiles into output image
        let mut rgb = vec![0u8; out_w * out_h * 3];
        for tile_row in 0..rows {
            for tile_col in 0..cols {
                let idx = tile_row * cols + tile_col;
                if idx >= tile_rgbs.len() { break; }
                let (ref tile_rgb, tw, th) = tile_rgbs[idx];
                let tw = tw as usize;
                let th = th as usize;
                let ox = tile_col * tw;
                let oy = tile_row * th;

                for ty in 0..th {
                    let dy = oy + ty;
                    if dy >= out_h { break; }
                    for tx in 0..tw {
                        let dx = ox + tx;
                        if dx >= out_w { break; }
                        let src = (ty * tw + tx) * 3;
                        let dst = (dy * out_w + dx) * 3;
                        rgb[dst..dst + 3].copy_from_slice(&tile_rgb[src..src + 3]);
                    }
                }
            }
        }

        Ok(AgnoImage::new(rgb, out_w as u64, out_h as u64, exif))
    }
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
