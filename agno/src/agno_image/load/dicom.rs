// The disabled-feature error lives in one place: the `#[cfg(not(feature =
// "dicom"))]` arm of the loader/FFI dispatch. This module compiles to nothing
// when the feature is off.
#[cfg(feature = "dicom")]
use std::error::Error;

#[cfg(feature = "dicom")]
use crate::{agno_image::AgnoImage, exif::ExifContext};

/// Decode DICOM bytes into an `AgnoImage` (frame 0). `page_count` is set to the
/// DICOM `NumberOfFrames` (1 for ordinary single-frame slices). Use
/// [`load_dicom_frame_from_bytes`] to composite a specific frame.
#[cfg(feature = "dicom")]
pub fn load_dicom_from_bytes(data: &[u8], exif: ExifContext) -> Result<AgnoImage, Box<dyn Error>> {
    load_dicom_frame_from_bytes(data, 0, exif)
}

/// Decode a specific frame (0-based) of a DICOM object into an `AgnoImage`.
#[cfg(feature = "dicom")]
pub fn load_dicom_frame_from_bytes(
    data: &[u8],
    frame_index: usize,
    exif: ExifContext,
) -> Result<AgnoImage, Box<dyn Error>> {
    let (rgb, width, height, frames) = crate::codec::dicom::decode_dicom_frame(data, frame_index)?;
    let mut img = AgnoImage::new(rgb, width as u64, height as u64, exif);
    img.set_page_count(frames as u64);
    Ok(img)
}

#[cfg(test)]
#[cfg(feature = "dicom")]
mod tests {
    use super::*;
    use crate::codec::dicom::test_fixtures::{make_part10, pad, short, us};

    // Minimal 2x2 16-bit MONOCHROME2 Part-10 file.
    fn make_min_dicom() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&short(0x0028, 0x0002, b"US", &us(1)));
        d.extend_from_slice(&short(
            0x0028,
            0x0004,
            b"CS",
            &pad(b"MONOCHROME2".to_vec(), b' '),
        ));
        d.extend_from_slice(&short(0x0028, 0x0010, b"US", &us(2)));
        d.extend_from_slice(&short(0x0028, 0x0011, b"US", &us(2)));
        d.extend_from_slice(&short(0x0028, 0x0100, b"US", &us(16)));
        d.extend_from_slice(&short(0x0028, 0x0101, b"US", &us(16)));
        d.extend_from_slice(&short(0x0028, 0x0102, b"US", &us(15)));
        d.extend_from_slice(&short(0x0028, 0x0103, b"US", &us(0)));
        d.extend_from_slice(&short(0x0028, 0x1050, b"DS", &pad(b"128".to_vec(), b' ')));
        d.extend_from_slice(&short(0x0028, 0x1051, b"DS", &pad(b"256".to_vec(), b' ')));
        d.extend_from_slice(&short(0x0028, 0x1052, b"DS", &pad(b"0".to_vec(), b' ')));
        d.extend_from_slice(&short(0x0028, 0x1053, b"DS", &pad(b"1".to_vec(), b' ')));
        let pixels: Vec<u8> = [0u16, 64, 128, 255]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        make_part10(&d, &pixels)
    }

    #[test]
    fn bridge_builds_agno_image() {
        let bytes = make_min_dicom();
        let img = load_dicom_from_bytes(&bytes, ExifContext::default()).unwrap();
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        assert_eq!(img.page_count, 1);
        assert_eq!(img.as_slice().len(), 2 * 2 * 3);
    }
}
