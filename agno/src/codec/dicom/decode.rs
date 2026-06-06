//! DICOM pixel extraction and RGB8 rendering.

use std::error::Error;

use super::parse::{DicomImage, Photometric, parse_dicom};
use super::voi::{auto_window, modality, window_to_u8};

/// Decode DICOM bytes to RGB8. Returns `(rgb8, width, height, frame_count)`.
#[allow(clippy::type_complexity)]
pub fn decode_dicom(data: &[u8]) -> Result<(Vec<u8>, u32, u32, usize), Box<dyn Error>> {
    let img = parse_dicom(data)?;
    let w = img.columns as usize;
    let h = img.rows as usize;
    let rgb = render_frame0(&img, w, h)?;
    Ok((
        rgb,
        img.columns as u32,
        img.rows as u32,
        img.number_of_frames,
    ))
}

/// Read `count` stored samples from `frame`, applying BitsStored masking
/// (unsigned) or sign-extension (signed), returning them as f64.
fn read_stored_values(frame: &[u8], count: usize, img: &DicomImage) -> Vec<f64> {
    let signed = img.pixel_representation == 1;
    let bits_stored = img.bits_stored.max(1) as u32;
    let mut out = Vec::with_capacity(count);
    if img.bits_allocated == 16 {
        for i in 0..count {
            let raw = u16::from_le_bytes([frame[i * 2], frame[i * 2 + 1]]);
            let v = if signed {
                let shift = 16u32.saturating_sub(bits_stored);
                f64::from((raw << shift) as i16 >> shift)
            } else {
                let mask = if bits_stored >= 16 {
                    0xFFFF
                } else {
                    (1u16 << bits_stored) - 1
                };
                f64::from(raw & mask)
            };
            out.push(v);
        }
    } else {
        // bits_allocated == 8
        for &raw in frame.iter().take(count) {
            let v = if signed {
                let shift = 8u32.saturating_sub(bits_stored);
                f64::from((raw << shift) as i8 >> shift)
            } else {
                let mask: u8 = if bits_stored >= 8 {
                    0xFF
                } else {
                    ((1u16 << bits_stored) - 1) as u8
                };
                f64::from(raw & mask)
            };
            out.push(v);
        }
    }
    out
}

fn render_rgb(frame: &[u8], n: usize, planar: u16) -> Vec<u8> {
    if planar == 0 {
        frame[..n * 3].to_vec()
    } else {
        let mut out = vec![0u8; n * 3];
        for i in 0..n {
            out[i * 3] = frame[i];
            out[i * 3 + 1] = frame[n + i];
            out[i * 3 + 2] = frame[2 * n + i];
        }
        out
    }
}

fn render_frame0(img: &DicomImage, w: usize, h: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    let bps = (img.bits_allocated / 8) as usize;
    let spp = img.samples_per_pixel as usize;
    let n = w * h;
    let frame_bytes = n * spp * bps;
    if img.pixel_data.len() < frame_bytes {
        return Err(format!(
            "DICOM pixel data too short: have {}, need {}",
            img.pixel_data.len(),
            frame_bytes
        )
        .into());
    }
    let frame = &img.pixel_data[..frame_bytes];

    if img.photometric == Photometric::Rgb && spp == 3 && bps == 1 {
        return Ok(render_rgb(frame, n, img.planar_configuration));
    }
    if spp != 1 {
        return Err(format!(
            "unsupported DICOM pixel format: samples_per_pixel={}, photometric={:?}",
            spp, img.photometric
        )
        .into());
    }

    let stored = read_stored_values(frame, n, img);
    let (center, width) = match (img.window_center, img.window_width) {
        (Some(c), Some(wd)) => (c, wd),
        _ => {
            let mut min = f64::INFINITY;
            let mut max = f64::NEG_INFINITY;
            for &s in &stored {
                let m = modality(s, img.rescale_slope, img.rescale_intercept);
                min = min.min(m);
                max = max.max(m);
            }
            auto_window(min, max)
        }
    };
    let invert = img.photometric == Photometric::Monochrome1;

    let mut out = Vec::with_capacity(n * 3);
    for &s in &stored {
        let m = modality(s, img.rescale_slope, img.rescale_intercept);
        let mut v = window_to_u8(m, center, width);
        if invert {
            v = 255 - v;
        }
        out.push(v);
        out.push(v);
        out.push(v);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Re-export the same Part-10 builder helpers used in parse.rs tests by
    // constructing files inline here.
    fn short(group: u16, elem: u16, vr: &[u8; 2], val: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&group.to_le_bytes());
        v.extend_from_slice(&elem.to_le_bytes());
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
    fn us(v: u16) -> Vec<u8> {
        v.to_le_bytes().to_vec()
    }
    fn make_part10(dataset: &[u8], pixel: &[u8]) -> Vec<u8> {
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
        out.extend_from_slice(dataset);
        out.extend_from_slice(&0x7FE0u16.to_le_bytes());
        out.extend_from_slice(&0x0010u16.to_le_bytes());
        out.extend_from_slice(b"OW");
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&(pixel.len() as u32).to_le_bytes());
        out.extend_from_slice(pixel);
        out
    }

    // Build a 2x2 grayscale dataset; `photometric` is "MONOCHROME1"/"MONOCHROME2".
    fn gray_dataset(
        photometric: &[u8],
        bits_alloc: u16,
        bits_stored: u16,
        pixel_rep: u16,
        wc: &[u8],
        ww: &[u8],
    ) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&short(0x0028, 0x0002, b"US", &us(1)));
        d.extend_from_slice(&short(
            0x0028,
            0x0004,
            b"CS",
            &pad(photometric.to_vec(), b' '),
        ));
        d.extend_from_slice(&short(0x0028, 0x0010, b"US", &us(2)));
        d.extend_from_slice(&short(0x0028, 0x0011, b"US", &us(2)));
        d.extend_from_slice(&short(0x0028, 0x0100, b"US", &us(bits_alloc)));
        d.extend_from_slice(&short(0x0028, 0x0101, b"US", &us(bits_stored)));
        d.extend_from_slice(&short(0x0028, 0x0102, b"US", &us(bits_stored - 1)));
        d.extend_from_slice(&short(0x0028, 0x0103, b"US", &us(pixel_rep)));
        if !wc.is_empty() {
            d.extend_from_slice(&short(0x0028, 0x1050, b"DS", &pad(wc.to_vec(), b' ')));
            d.extend_from_slice(&short(0x0028, 0x1051, b"DS", &pad(ww.to_vec(), b' ')));
        }
        d.extend_from_slice(&short(0x0028, 0x1052, b"DS", &pad(b"0".to_vec(), b' ')));
        d.extend_from_slice(&short(0x0028, 0x1053, b"DS", &pad(b"1".to_vec(), b' ')));
        d
    }

    #[test]
    fn mono2_16bit_windows_to_expected_gray() {
        // center=128, width=256 => value 0->0, 128->128, 255->255, 64->64.
        let pixels: Vec<u8> = [0u16, 64, 128, 255]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let ds = gray_dataset(b"MONOCHROME2", 16, 16, 0, b"128", b"256");
        let file = make_part10(&ds, &pixels);
        let (rgb, w, h, frames) = decode_dicom(&file).unwrap();
        assert_eq!((w, h, frames), (2, 2, 1));
        assert_eq!(rgb.len(), 2 * 2 * 3);
        // pixel 0 -> 0, pixel 1 -> 64, pixel 2 -> 128, pixel 3 -> 255 (replicated)
        assert_eq!(&rgb[0..3], &[0, 0, 0]);
        assert_eq!(&rgb[3..6], &[64, 64, 64]);
        assert_eq!(&rgb[6..9], &[128, 128, 128]);
        assert_eq!(&rgb[9..12], &[255, 255, 255]);
    }

    #[test]
    fn mono1_is_inverted() {
        let pixels: Vec<u8> = [0u16, 255, 0, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let ds = gray_dataset(b"MONOCHROME1", 16, 16, 0, b"128", b"256");
        let file = make_part10(&ds, &pixels);
        let (rgb, _, _, _) = decode_dicom(&file).unwrap();
        // MONOCHROME1 inverts: value 0 -> 255, value 255 -> 0.
        assert_eq!(&rgb[0..3], &[255, 255, 255]);
        assert_eq!(&rgb[3..6], &[0, 0, 0]);
    }

    #[test]
    fn eight_bit_unsigned_decodes() {
        let pixels = [0u8, 64, 128, 255];
        let ds = gray_dataset(b"MONOCHROME2", 8, 8, 0, b"128", b"256");
        let file = make_part10(&ds, &pixels);
        let (rgb, _, _, _) = decode_dicom(&file).unwrap();
        assert_eq!(&rgb[0..3], &[0, 0, 0]);
        assert_eq!(&rgb[9..12], &[255, 255, 255]);
    }

    #[test]
    fn auto_window_used_when_voi_absent() {
        // No WindowCenter/Width: auto-window spans the modality min/max so the
        // smallest sample -> 0 and the largest -> 255.
        let pixels: Vec<u8> = [0u16, 100, 200, 400]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let ds = gray_dataset(b"MONOCHROME2", 16, 16, 0, b"", b"");
        let file = make_part10(&ds, &pixels);
        let (rgb, _, _, _) = decode_dicom(&file).unwrap();
        assert_eq!(rgb[0], 0); // min sample -> black
        assert_eq!(rgb[9], 255); // max sample -> white
    }

    #[test]
    fn signed_16bit_sign_extends() {
        // bits_stored=12, signed: 0xFFF (= -1) should be below a window centered
        // on 0, mapping toward black; +1 should be above center.
        let pixels: Vec<u8> = [0x0FFFu16, 0x0001, 0x0000, 0x0000]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let ds = gray_dataset(b"MONOCHROME2", 16, 12, 1, b"0", b"4");
        let file = make_part10(&ds, &pixels);
        let (rgb, _, _, _) = decode_dicom(&file).unwrap();
        // value -1 maps darker than value +1.
        assert!(
            rgb[0] < rgb[3],
            "expected -1 darker than +1: {} {}",
            rgb[0],
            rgb[3]
        );
    }

    fn implicit_elem(g: u16, e: u16, val: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&g.to_le_bytes());
        v.extend_from_slice(&e.to_le_bytes());
        v.extend_from_slice(&(val.len() as u32).to_le_bytes());
        v.extend_from_slice(val);
        v
    }

    fn rgb_dataset(planar: u16) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&short(0x0028, 0x0002, b"US", &us(3)));
        d.extend_from_slice(&short(0x0028, 0x0004, b"CS", &pad(b"RGB".to_vec(), b' ')));
        d.extend_from_slice(&short(0x0028, 0x0006, b"US", &us(planar)));
        d.extend_from_slice(&short(0x0028, 0x0010, b"US", &us(1)));
        d.extend_from_slice(&short(0x0028, 0x0011, b"US", &us(2)));
        d.extend_from_slice(&short(0x0028, 0x0100, b"US", &us(8)));
        d.extend_from_slice(&short(0x0028, 0x0101, b"US", &us(8)));
        d.extend_from_slice(&short(0x0028, 0x0102, b"US", &us(7)));
        d.extend_from_slice(&short(0x0028, 0x0103, b"US", &us(0)));
        d
    }

    #[test]
    fn rgb_interleaved_passes_through() {
        let pixels = [10u8, 20, 30, 40, 50, 60]; // 2x1 RGBRGB
        let file = make_part10(&rgb_dataset(0), &pixels);
        let (rgb, w, h, _) = decode_dicom(&file).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgb, vec![10, 20, 30, 40, 50, 60]);
    }

    #[test]
    fn rgb_planar_deinterleaves() {
        // planar=1: R plane [10,40], G plane [20,50], B plane [30,60]
        let pixels = [10u8, 40, 20, 50, 30, 60];
        let file = make_part10(&rgb_dataset(1), &pixels);
        let (rgb, _, _, _) = decode_dicom(&file).unwrap();
        assert_eq!(rgb, vec![10, 20, 30, 40, 50, 60]);
    }

    #[test]
    fn malformed_bits_stored_exceeding_allocated_does_not_panic() {
        // BitsStored (12) > BitsAllocated (8) is structurally invalid DICOM.
        // The renderer must not panic on it (signed path shift underflow).
        let pixels = [0u8, 64, 200, 255];
        let ds = gray_dataset(b"MONOCHROME2", 8, 12, 1, b"128", b"256");
        let file = make_part10(&ds, &pixels);
        let (rgb, w, h, _) = decode_dicom(&file).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgb.len(), 2 * 2 * 3);
    }

    #[test]
    fn implicit_vr_mono16_decodes() {
        // Implicit VR LE: dataset elements are tag(4) + len u32(4) + value (no VR).
        let mut d = Vec::new();
        d.extend_from_slice(&implicit_elem(0x0028, 0x0002, &us(1)));
        d.extend_from_slice(&implicit_elem(
            0x0028,
            0x0004,
            &pad(b"MONOCHROME2".to_vec(), b' '),
        ));
        d.extend_from_slice(&implicit_elem(0x0028, 0x0010, &us(2)));
        d.extend_from_slice(&implicit_elem(0x0028, 0x0011, &us(2)));
        d.extend_from_slice(&implicit_elem(0x0028, 0x0100, &us(16)));
        d.extend_from_slice(&implicit_elem(0x0028, 0x0101, &us(16)));
        d.extend_from_slice(&implicit_elem(0x0028, 0x0102, &us(15)));
        d.extend_from_slice(&implicit_elem(0x0028, 0x0103, &us(0)));
        d.extend_from_slice(&implicit_elem(0x0028, 0x1050, &pad(b"128".to_vec(), b' ')));
        d.extend_from_slice(&implicit_elem(0x0028, 0x1051, &pad(b"256".to_vec(), b' ')));
        d.extend_from_slice(&implicit_elem(0x0028, 0x1052, &pad(b"0".to_vec(), b' ')));
        d.extend_from_slice(&implicit_elem(0x0028, 0x1053, &pad(b"1".to_vec(), b' ')));
        let pixels: Vec<u8> = [0u16, 64, 128, 255]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();

        // File meta is always Explicit VR LE; only the dataset is Implicit VR LE.
        let mut out = vec![0u8; 128];
        out.extend_from_slice(b"DICM");
        let ts = pad(b"1.2.840.10008.1.2".to_vec(), 0);
        let meta = short(0x0002, 0x0010, b"UI", &ts);
        out.extend_from_slice(&short(
            0x0002,
            0x0000,
            b"UL",
            &(meta.len() as u32).to_le_bytes(),
        ));
        out.extend_from_slice(&meta);
        out.extend_from_slice(&d);
        out.extend_from_slice(&implicit_elem(0x7FE0, 0x0010, &pixels));

        let (rgb, w, h, frames) = decode_dicom(&out).unwrap();
        assert_eq!((w, h, frames), (2, 2, 1));
        assert_eq!(&rgb[0..3], &[0, 0, 0]);
        assert_eq!(&rgb[3..6], &[64, 64, 64]);
        assert_eq!(&rgb[9..12], &[255, 255, 255]);
    }
}
