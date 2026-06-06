use std::error::Error;

use crate::{agno_image::AgnoImage, exif::ExifContext};

/// Decode DICOM bytes into an `AgnoImage`. `page_count` is set to the DICOM
/// `NumberOfFrames` (1 for ordinary single-frame slices). Only frame 0 is
/// composited into the returned image.
#[cfg(feature = "dicom")]
pub fn load_dicom_from_bytes(data: &[u8], exif: ExifContext) -> Result<AgnoImage, Box<dyn Error>> {
    let (rgb, width, height, frames) = crate::codec::dicom::decode_dicom(data)?;
    let mut img = AgnoImage::new(rgb, width as u64, height as u64, exif);
    img.set_page_count(frames as u64);
    Ok(img)
}

#[cfg(not(feature = "dicom"))]
pub fn load_dicom_from_bytes(
    _data: &[u8],
    _exif: ExifContext,
) -> Result<AgnoImage, Box<dyn Error>> {
    Err("DICOM support is not enabled. Please enable the 'dicom' feature.".into())
}

#[cfg(test)]
#[cfg(feature = "dicom")]
mod tests {
    use super::*;

    // Minimal 2x2 16-bit MONOCHROME2 Part-10 file built inline.
    fn make_min_dicom() -> Vec<u8> {
        fn short(g: u16, e: u16, vr: &[u8; 2], val: &[u8]) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&g.to_le_bytes());
            v.extend_from_slice(&e.to_le_bytes());
            v.extend_from_slice(vr);
            v.extend_from_slice(&(val.len() as u16).to_le_bytes());
            v.extend_from_slice(val);
            v
        }
        fn pad(mut s: Vec<u8>, p: u8) -> Vec<u8> {
            if !s.len().is_multiple_of(2) {
                s.push(p);
            }
            s
        }
        let us = |v: u16| v.to_le_bytes().to_vec();
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

        let mut out = vec![0u8; 128];
        out.extend_from_slice(b"DICM");
        let ts = pad(b"1.2.840.10008.1.2.1".to_vec(), 0);
        let meta = short(0x0002, 0x0010, b"UI", &ts);
        out.extend_from_slice(&short(
            0x0002,
            0x0000,
            b"UL",
            &(meta.len() as u32).to_le_bytes(),
        ));
        out.extend_from_slice(&meta);
        out.extend_from_slice(&d);
        let pixels: Vec<u8> = [0u16, 64, 128, 255]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        out.extend_from_slice(&0x7FE0u16.to_le_bytes());
        out.extend_from_slice(&0x0010u16.to_le_bytes());
        out.extend_from_slice(b"OW");
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&(pixels.len() as u32).to_le_bytes());
        out.extend_from_slice(&pixels);
        out
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
