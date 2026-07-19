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
        Ok(AgnoImage::new(
            rgb,
            picture.width as u64,
            picture.height as u64,
            exif,
        ))
    } else {
        // Grid image: decode each tile and stitch
        let out_w = heif.width as usize;
        let out_h = heif.height as usize;
        let cols = heif.grid_cols as usize;
        let rows = heif.grid_rows as usize;

        // out_w/out_h come from the untrusted ispe box; reject before
        // decoding (and buffering) any tile.
        crate::guard::check_dims(out_w as u64, out_h as u64)?;

        // Tiles beyond rows*cols are never stitched, so never decode them.
        let tile_count = cols.saturating_mul(rows);

        // Decode tiles, checking grid geometry against the first tile
        // (all tiles share the same coded size) before decoding the rest.
        // This bounds total buffered tile memory to roughly the output
        // canvas size, which check_dims above already bounds.
        let mut tile_rgbs: Vec<(Vec<u8>, u32, u32)> = Vec::with_capacity(heif.tiles.len());
        for (idx, tile_data) in heif.tiles.iter().take(tile_count).enumerate() {
            let pic = decode_hevc_still(&heif.hvcc, tile_data)?;
            let w = pic.width;
            let h = pic.height;
            if idx == 0 {
                check_grid_geometry(out_w, out_h, cols, rows, w as usize, h as usize)?;
            }
            tile_rgbs.push((pic.to_rgb8(), w, h));
        }

        let rgb = stitch_grid(&tile_rgbs, out_w, out_h, cols, rows)?;

        Ok(AgnoImage::new(rgb, out_w as u64, out_h as u64, exif))
    }
}

/// Validate that a grid of `cols` x `rows` tiles of `tile_w` x `tile_h` is
/// consistent with a declared `out_w` x `out_h` output canvas. Per HEIF grid
/// semantics, the grid covers the canvas with less than one tile of overhang
/// on each axis: `(cols-1) * tile_w < out_w` and `(rows-1) * tile_h < out_h`.
/// Rejecting inconsistent geometry here (after the first tile decodes) bounds
/// how many further tiles get decoded and buffered before stitching.
fn check_grid_geometry(
    out_w: usize,
    out_h: usize,
    cols: usize,
    rows: usize,
    tile_w: usize,
    tile_h: usize,
) -> Result<(), Box<dyn Error>> {
    let covered_w = cols.saturating_sub(1).saturating_mul(tile_w);
    let covered_h = rows.saturating_sub(1).saturating_mul(tile_h);
    if covered_w < out_w && covered_h < out_h {
        Ok(())
    } else {
        Err(format!(
            "HEIC grid geometry {cols}x{rows} tiles of {tile_w}x{tile_h} inconsistent with output {out_w}x{out_h}"
        )
        .into())
    }
}

/// Stitch decoded grid tiles into a single RGB8 canvas of `out_w` x `out_h`.
fn stitch_grid(
    tile_rgbs: &[(Vec<u8>, u32, u32)],
    out_w: usize,
    out_h: usize,
    cols: usize,
    rows: usize,
) -> Result<Vec<u8>, Box<dyn Error>> {
    crate::guard::check_dims(out_w as u64, out_h as u64)?;
    let mut rgb = crate::guard::try_vec(0u8, out_w * out_h * 3)?;
    for tile_row in 0..rows {
        for tile_col in 0..cols {
            let idx = tile_row * cols + tile_col;
            if idx >= tile_rgbs.len() {
                break;
            }
            let (ref tile_rgb, tw, th) = tile_rgbs[idx];
            let tw = tw as usize;
            let th = th as usize;
            let ox = tile_col * tw;
            let oy = tile_row * th;

            for ty in 0..th {
                let dy = oy + ty;
                if dy >= out_h {
                    break;
                }
                for tx in 0..tw {
                    let dx = ox + tx;
                    if dx >= out_w {
                        break;
                    }
                    let src = (ty * tw + tx) * 3;
                    let dst = (dy * out_w + dx) * 3;
                    rgb[dst..dst + 3].copy_from_slice(&tile_rgb[src..src + 3]);
                }
            }
        }
    }
    Ok(rgb)
}

#[cfg(test)]
mod tests {
    use crate::agno_image::load::{detect_image_type, load_agno_image_from_file};
    use crate::exif::spec;
    use std::fs::File;

    /// Grid output dimensions come from the untrusted ispe/grid descriptor and
    /// must be rejected before the canvas is allocated from them.
    #[test]
    fn stitch_grid_rejects_absurd_output_dimensions() {
        let tile = (vec![0u8; 16 * 16 * 3], 16u32, 16u32);
        assert!(super::stitch_grid(&[tile], 1 << 21, 1 << 21, 1, 1).is_err());
    }

    /// A consistent grid covers the output canvas with less than one tile of
    /// overhang on each axis (HEIF grid semantics). sideways2.heic is 8x6
    /// tiles of 512x512 covering a 4032x3024 canvas.
    #[test]
    fn check_grid_geometry_accepts_real_grid_shape() {
        super::check_grid_geometry(4032, 3024, 8, 6, 512, 512)
            .expect("sideways2.heic grid geometry should be accepted");
    }

    /// An inflated tile count/size relative to a tiny declared output must be
    /// rejected before further tiles are decoded and buffered.
    #[test]
    fn check_grid_geometry_rejects_inflated_grid() {
        assert!(super::check_grid_geometry(100, 100, 100, 100, 512, 512).is_err());
    }

    #[test]
    fn detect_heic_format() {
        let mut file = File::open("../tests/data/test-heic.heic").unwrap();
        let image_type = detect_image_type(&mut file).unwrap();
        assert!(matches!(
            image_type,
            crate::agno_image::load::ImageType::Heic
        ));
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
        assert!(
            img.exif.get_tag_value(spec::OFFSET_TIME).is_some(),
            "Must follow ExifIFD sub-IFD to find OffsetTime"
        );
        assert!(
            img.exif.get_tag_value(spec::GPS_LATITUDE_REF).is_some(),
            "Must follow GPS sub-IFD to find GPSLatitudeRef"
        );
    }
}
