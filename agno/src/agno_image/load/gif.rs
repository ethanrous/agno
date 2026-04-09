use std::error::Error;

use crate::{agno_image::AgnoImage, exif::ExifContext};

/// Load a specific frame from GIF bytes as an `AgnoImage`.
///
/// `frame_index` is 0-based. The returned image's `page_count` is the total
/// frame count of the GIF. Frames after the requested one are still parsed
/// (to count them) but not composited.
#[cfg(feature = "gif")]
pub fn load_gif_frame_from_bytes(
    data: &[u8],
    frame_index: usize,
    exif: ExifContext,
) -> Result<AgnoImage, Box<dyn Error>> {
    let (rgb, width, height, frame_count) = crate::codec::gif::decode_gif_frame(data, frame_index)?;
    let mut img = AgnoImage::new(rgb, width as u64, height as u64, exif);
    img.set_page_count(frame_count as u64);
    Ok(img)
}

#[cfg(not(feature = "gif"))]
pub fn load_gif_frame_from_bytes(
    _data: &[u8],
    _frame_index: usize,
    _exif: ExifContext,
) -> Result<AgnoImage, Box<dyn Error>> {
    Err("GIF support is not enabled. Please enable the 'gif' feature.".into())
}

#[cfg(test)]
#[cfg(feature = "gif")]
mod tests {
    use super::*;
    use image::{Frame, RgbaImage, codecs::gif::GifEncoder};
    use std::io::Cursor;

    fn make_gif(frame_count: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut enc = GifEncoder::new(Cursor::new(&mut bytes));
            for i in 0..frame_count {
                let mut rgba = RgbaImage::new(1, 1);
                rgba.put_pixel(0, 0, image::Rgba([(i * 80) as u8, 100, 50, 255]));
                enc.encode_frame(Frame::new(rgba)).unwrap();
            }
        }
        bytes
    }

    #[test]
    fn loads_first_frame_with_correct_page_count() {
        let bytes = make_gif(3);
        let img = load_gif_frame_from_bytes(&bytes, 0, ExifContext::default()).unwrap();
        assert!(img.width > 0 && img.height > 0);
        assert_eq!(img.page_count, 3);
    }

    #[test]
    fn loads_specific_frame() {
        let bytes = make_gif(3);
        for i in 0..3 {
            let img = load_gif_frame_from_bytes(&bytes, i, ExifContext::default()).unwrap();
            assert_eq!(img.page_count, 3);
        }
    }

    #[test]
    fn frame_out_of_range_errors() {
        let bytes = make_gif(2);
        let result = load_gif_frame_from_bytes(&bytes, 5, ExifContext::default());
        assert!(result.is_err());
    }
}
