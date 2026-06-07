//! DICOM pixel extraction and RGB8 rendering.

use std::error::Error;

use super::parse::{DicomImage, Photometric, parse_dicom};
use super::voi::{auto_window, modality, window_to_u8};

/// Decode DICOM bytes to RGB8 (frame 0). Returns `(rgb8, width, height, frame_count)`.
#[allow(clippy::type_complexity)]
pub fn decode_dicom(data: &[u8]) -> Result<(Vec<u8>, u32, u32, usize), Box<dyn Error>> {
    decode_dicom_frame(data, 0)
}

/// Decode a specific frame (0-based) of a DICOM object to RGB8.
/// Returns `(rgb8, width, height, frame_count)`.
#[allow(clippy::type_complexity)]
pub fn decode_dicom_frame(
    data: &[u8],
    frame_index: usize,
) -> Result<(Vec<u8>, u32, u32, usize), Box<dyn Error>> {
    let img = parse_dicom(data)?;
    let rgb = render_frame(&img, frame_index)?;
    Ok((
        rgb,
        img.columns as u32,
        img.rows as u32,
        img.number_of_frames,
    ))
}

/// Read stored sample `i` from `frame` (8- or 16-bit), applying BitsStored
/// masking (unsigned) or sign-extension (signed), returning it as f64.
fn stored_value(
    frame: &[u8],
    i: usize,
    signed: bool,
    bits_allocated: u16,
    bits_stored: u32,
) -> f64 {
    if bits_allocated == 16 {
        let raw = u16::from_le_bytes([frame[i * 2], frame[i * 2 + 1]]);
        if signed {
            let shift = 16u32.saturating_sub(bits_stored);
            f64::from(((raw << shift) as i16) >> shift)
        } else {
            let mask = if bits_stored >= 16 {
                0xFFFF
            } else {
                (1u16 << bits_stored) - 1
            };
            f64::from(raw & mask)
        }
    } else {
        let raw = frame[i];
        if signed {
            let shift = 8u32.saturating_sub(bits_stored);
            f64::from(((raw << shift) as i8) >> shift)
        } else {
            let mask: u8 = if bits_stored >= 8 {
                0xFF
            } else {
                ((1u16 << bits_stored) - 1) as u8
            };
            f64::from(raw & mask)
        }
    }
}

/// Render an RGB frame to RGB8. Channels are masked to BitsStored and scaled to
/// 8 bits (down for 16-bit, up for sub-8-bit), matching the grayscale path.
fn render_rgb(
    frame: &[u8],
    n: usize,
    planar: u16,
    bits_allocated: u16,
    bits_stored: u32,
) -> Vec<u8> {
    let channel = |sample: usize| -> u8 {
        if bits_allocated == 16 {
            let raw = u16::from_le_bytes([frame[sample * 2], frame[sample * 2 + 1]]);
            let masked = if bits_stored >= 16 {
                raw
            } else {
                raw & ((1u16 << bits_stored) - 1)
            };
            if bits_stored > 8 {
                (masked >> (bits_stored - 8)) as u8
            } else {
                (masked << (8 - bits_stored)) as u8
            }
        } else {
            let raw = frame[sample];
            if bits_stored >= 8 {
                raw
            } else {
                (raw & (((1u16 << bits_stored) - 1) as u8)) << (8 - bits_stored)
            }
        }
    };
    let mut out = vec![0u8; n * 3];
    for i in 0..n {
        let (r, g, b) = if planar == 0 {
            (i * 3, i * 3 + 1, i * 3 + 2) // interleaved RGBRGB
        } else {
            (i, n + i, 2 * n + i) // planar: R plane, G plane, B plane
        };
        out[i * 3] = channel(r);
        out[i * 3 + 1] = channel(g);
        out[i * 3 + 2] = channel(b);
    }
    out
}

fn render_frame(img: &DicomImage, frame_index: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    let w = img.columns as usize;
    let h = img.rows as usize;
    let bps = (img.bits_allocated / 8) as usize;
    let spp = img.samples_per_pixel as usize;
    let n = w * h;
    let frame_bytes = n * spp * bps;

    if frame_index >= img.number_of_frames {
        return Err(format!(
            "DICOM frame {frame_index} out of range (frame_count {})",
            img.number_of_frames
        )
        .into());
    }
    let start = frame_index * frame_bytes;
    let end = start + frame_bytes;
    if img.pixel_data.len() < end {
        return Err(format!(
            "DICOM pixel data too short: have {}, need {end} for frame {frame_index}",
            img.pixel_data.len()
        )
        .into());
    }
    let frame = &img.pixel_data[start..end];

    match img.photometric {
        Photometric::Rgb if spp == 3 => {
            // PlanarConfiguration is defined only for 0 (interleaved) or 1
            // (planar); reject other values rather than silently mis-decoding.
            if img.planar_configuration > 1 {
                return Err(format!(
                    "unsupported DICOM PlanarConfiguration: {}",
                    img.planar_configuration
                )
                .into());
            }
            Ok(render_rgb(
                frame,
                n,
                img.planar_configuration,
                img.bits_allocated,
                img.bits_stored as u32,
            ))
        }
        Photometric::Monochrome1 | Photometric::Monochrome2 if spp == 1 => {
            Ok(render_mono(img, frame, n))
        }
        other => Err(format!(
            "unsupported DICOM pixel format: photometric={other:?}, samples_per_pixel={spp}"
        )
        .into()),
    }
}

/// Render a single-sample MONOCHROME1/2 frame to grayscale RGB8 in one pass over
/// the sample bytes (no intermediate per-pixel buffer).
fn render_mono(img: &DicomImage, frame: &[u8], n: usize) -> Vec<u8> {
    let signed = img.pixel_representation == 1;
    let bits_allocated = img.bits_allocated;
    let bits_stored = img.bits_stored as u32; // build() guarantees >= 1
    let slope = img.rescale_slope;
    let intercept = img.rescale_intercept;
    let value = |i: usize| {
        modality(
            stored_value(frame, i, signed, bits_allocated, bits_stored),
            slope,
            intercept,
        )
    };

    let (center, width) = match (img.window_center, img.window_width) {
        (Some(c), Some(wd)) => (c, wd),
        _ => {
            // No VOI: auto-window the modality min/max. modality is affine in the
            // stored value, so derive it from the raw sample extremes instead of
            // running modality over every pixel.
            let mut min_s = f64::INFINITY;
            let mut max_s = f64::NEG_INFINITY;
            for i in 0..n {
                let s = stored_value(frame, i, signed, bits_allocated, bits_stored);
                min_s = min_s.min(s);
                max_s = max_s.max(s);
            }
            let a = modality(min_s, slope, intercept);
            let b = modality(max_s, slope, intercept);
            auto_window(a.min(b), a.max(b))
        }
    };
    let invert = img.photometric == Photometric::Monochrome1;

    let mut out = Vec::with_capacity(n * 3);
    for i in 0..n {
        let mut v = window_to_u8(value(i), center, width);
        if invert {
            v = 255 - v;
        }
        out.push(v);
        out.push(v);
        out.push(v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::dicom::test_fixtures::{make_part10, pad, short, us};

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
    fn eight_bit_rgb_substored_masks_and_scales() {
        // RGB with BitsAllocated=8, BitsStored=4: each channel is masked to the low
        // 4 bits and scaled up to 8 bits (v << 4), matching the 16-bit/grayscale
        // paths. The high nibble must be discarded before scaling.
        let mut d = Vec::new();
        d.extend_from_slice(&short(0x0028, 0x0002, b"US", &us(3)));
        d.extend_from_slice(&short(0x0028, 0x0004, b"CS", &pad(b"RGB".to_vec(), b' ')));
        d.extend_from_slice(&short(0x0028, 0x0006, b"US", &us(0))); // interleaved
        d.extend_from_slice(&short(0x0028, 0x0010, b"US", &us(1))); // Rows
        d.extend_from_slice(&short(0x0028, 0x0011, b"US", &us(2))); // Columns
        d.extend_from_slice(&short(0x0028, 0x0100, b"US", &us(8))); // BitsAllocated
        d.extend_from_slice(&short(0x0028, 0x0101, b"US", &us(4))); // BitsStored
        d.extend_from_slice(&short(0x0028, 0x0102, b"US", &us(3))); // HighBit
        d.extend_from_slice(&short(0x0028, 0x0103, b"US", &us(0)));
        let pixels = [0xFFu8, 0x10, 0x01, 0x18, 0x00, 0x0F];
        let file = make_part10(&d, &pixels);
        let (rgb, w, h, _) = decode_dicom(&file).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgb, vec![240, 0, 16, 128, 0, 240]);
    }

    #[test]
    fn rejects_invalid_planar_configuration() {
        // PlanarConfiguration is defined only for 0 or 1; a value of 2 must error
        // rather than silently deinterleave the samples as planar.
        let pixels = [10u8, 20, 30, 40, 50, 60];
        let file = make_part10(&rgb_dataset(2), &pixels);
        let err = decode_dicom(&file).unwrap_err().to_string();
        assert!(err.contains("PlanarConfiguration"), "got: {err}");
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

    #[test]
    fn rejects_palette_color_photometric() {
        // PALETTE COLOR (spp=1, needs a lookup table) must error rather than
        // render the raw palette indices as grayscale.
        let ds = gray_dataset(b"PALETTE COLOR", 8, 8, 0, b"128", b"256");
        let pixels = [0u8, 64, 128, 255];
        let file = make_part10(&ds, &pixels);
        let err = decode_dicom(&file).unwrap_err().to_string();
        assert!(err.contains("unsupported"), "got: {err}");
    }

    #[test]
    fn sixteen_bit_rgb_decodes() {
        // RGB with BitsAllocated=16: each channel masked to BitsStored and scaled
        // to 8 bits (v >> 8 for bits_stored=16).
        let mut d = Vec::new();
        d.extend_from_slice(&short(0x0028, 0x0002, b"US", &us(3)));
        d.extend_from_slice(&short(0x0028, 0x0004, b"CS", &pad(b"RGB".to_vec(), b' ')));
        d.extend_from_slice(&short(0x0028, 0x0006, b"US", &us(0))); // interleaved
        d.extend_from_slice(&short(0x0028, 0x0010, b"US", &us(1))); // Rows
        d.extend_from_slice(&short(0x0028, 0x0011, b"US", &us(2))); // Columns
        d.extend_from_slice(&short(0x0028, 0x0100, b"US", &us(16))); // BitsAllocated
        d.extend_from_slice(&short(0x0028, 0x0101, b"US", &us(16))); // BitsStored
        d.extend_from_slice(&short(0x0028, 0x0102, b"US", &us(15))); // HighBit
        d.extend_from_slice(&short(0x0028, 0x0103, b"US", &us(0)));
        let pixels: Vec<u8> = [0x1000u16, 0x2000, 0x3000, 0x4000, 0x5000, 0x6000]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let file = make_part10(&d, &pixels);
        let (rgb, w, h, _) = decode_dicom(&file).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgb, vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60]);
    }

    #[test]
    fn decodes_selected_frame() {
        // A 2-frame MONOCHROME2 object: frame 1 must differ from frame 0.
        let mut d = Vec::new();
        d.extend_from_slice(&short(0x0028, 0x0002, b"US", &us(1)));
        d.extend_from_slice(&short(
            0x0028,
            0x0004,
            b"CS",
            &pad(b"MONOCHROME2".to_vec(), b' '),
        ));
        d.extend_from_slice(&short(0x0028, 0x0008, b"IS", &pad(b"2".to_vec(), b' '))); // NumberOfFrames
        d.extend_from_slice(&short(0x0028, 0x0010, b"US", &us(1))); // Rows
        d.extend_from_slice(&short(0x0028, 0x0011, b"US", &us(2))); // Columns
        d.extend_from_slice(&short(0x0028, 0x0100, b"US", &us(8)));
        d.extend_from_slice(&short(0x0028, 0x0101, b"US", &us(8)));
        d.extend_from_slice(&short(0x0028, 0x0102, b"US", &us(7)));
        d.extend_from_slice(&short(0x0028, 0x0103, b"US", &us(0)));
        d.extend_from_slice(&short(0x0028, 0x1050, b"DS", &pad(b"128".to_vec(), b' ')));
        d.extend_from_slice(&short(0x0028, 0x1051, b"DS", &pad(b"256".to_vec(), b' ')));
        let pixels = [10u8, 20, 200, 210]; // frame0 [10,20], frame1 [200,210]
        let file = make_part10(&d, &pixels);

        let (rgb0, _, _, frames) = decode_dicom(&file).unwrap();
        assert_eq!(frames, 2);
        assert_eq!(&rgb0[0..3], &[10, 10, 10]);
        assert_eq!(&rgb0[3..6], &[20, 20, 20]);

        let (rgb1, w, h, _) = decode_dicom_frame(&file, 1).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(&rgb1[0..3], &[200, 200, 200]);
        assert_eq!(&rgb1[3..6], &[210, 210, 210]);

        // Out-of-range frame errors rather than panicking or returning frame 0.
        assert!(decode_dicom_frame(&file, 2).is_err());
    }

    #[test]
    fn non_square_mono_keeps_width_and_height_distinct() {
        // Rows != Columns so a width/height transposition would be caught.
        let mut d = Vec::new();
        d.extend_from_slice(&short(0x0028, 0x0002, b"US", &us(1)));
        d.extend_from_slice(&short(
            0x0028,
            0x0004,
            b"CS",
            &pad(b"MONOCHROME2".to_vec(), b' '),
        ));
        d.extend_from_slice(&short(0x0028, 0x0010, b"US", &us(1))); // Rows = 1
        d.extend_from_slice(&short(0x0028, 0x0011, b"US", &us(4))); // Columns = 4
        d.extend_from_slice(&short(0x0028, 0x0100, b"US", &us(8)));
        d.extend_from_slice(&short(0x0028, 0x0101, b"US", &us(8)));
        d.extend_from_slice(&short(0x0028, 0x0102, b"US", &us(7)));
        d.extend_from_slice(&short(0x0028, 0x0103, b"US", &us(0)));
        d.extend_from_slice(&short(0x0028, 0x1050, b"DS", &pad(b"128".to_vec(), b' ')));
        d.extend_from_slice(&short(0x0028, 0x1051, b"DS", &pad(b"256".to_vec(), b' ')));
        let pixels = [0u8, 64, 128, 255];
        let file = make_part10(&d, &pixels);
        let (rgb, w, h, _) = decode_dicom(&file).unwrap();
        assert_eq!((w, h), (4, 1));
        assert_eq!(rgb.len(), 4 * 3);
    }

    #[test]
    fn nan_window_falls_back_to_auto() {
        // A non-finite WindowWidth/Center must not blacken the image; the decoder
        // falls back to auto-windowing the modality min/max.
        let pixels: Vec<u8> = [0u16, 100, 200, 400]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let ds = gray_dataset(b"MONOCHROME2", 16, 16, 0, b"nan", b"nan");
        let file = make_part10(&ds, &pixels);
        let (rgb, _, _, _) = decode_dicom(&file).unwrap();
        assert_eq!(rgb[0], 0); // min sample -> black
        assert_eq!(rgb[9], 255); // max sample -> white
    }
}
