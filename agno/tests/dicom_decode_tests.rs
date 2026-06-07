//! Integration tests for DICOM decoding via the public agno crate API.

#![cfg(feature = "dicom")]

use agno::agno_image::load::{load_agno_image_from_file, load_dicom_from_bytes};
use agno::codec::dicom::decode_dicom;
use agno::exif::ExifContext;

mod common;
use common::psnr;

const W: usize = 560;
const H: usize = 560;

#[test]
fn decodes_real_mri_matches_reference() {
    let data = std::fs::read("../tests/data/mri.dcm").unwrap();
    let (rgb, w, h, frames) = decode_dicom(&data).unwrap();
    assert_eq!((w as usize, h as usize), (W, H));
    assert_eq!(frames, 1);
    assert_eq!(rgb.len(), W * H * 3);

    let reference = std::fs::read("../tests/data/mri-reference.rgb").unwrap();
    assert_eq!(reference.len(), W * H * 3);
    let p = psnr(&rgb, &reference);
    assert!(p >= 45.0, "PSNR vs reference too low: {p:.2} dB");
}

#[test]
fn loader_entry_point_routes_dicom() {
    // The auto-detecting public loader must recognize and decode the .dcm file.
    let img = load_agno_image_from_file("../tests/data/mri.dcm").unwrap();
    assert_eq!(img.width, W as u64);
    assert_eq!(img.height, H as u64);
    assert_eq!(img.page_count, 1);
    assert_eq!(img.as_slice().len(), W * H * 3);
}

#[test]
fn bridge_decodes_real_file() {
    let data = std::fs::read("../tests/data/mri.dcm").unwrap();
    let img = load_dicom_from_bytes(&data, ExifContext::default()).unwrap();
    assert_eq!(img.width, W as u64);
    assert_eq!(img.height, H as u64);
}

#[test]
fn rejects_object_without_pixels() {
    // A Part-10 file with file meta but no pixel data element.
    let mut out = vec![0u8; 128];
    out.extend_from_slice(b"DICM");
    let mut ts = b"1.2.840.10008.1.2.1".to_vec();
    if !ts.len().is_multiple_of(2) {
        ts.push(0);
    }
    let mut meta = Vec::new();
    meta.extend_from_slice(&0x0002u16.to_le_bytes());
    meta.extend_from_slice(&0x0010u16.to_le_bytes());
    meta.extend_from_slice(b"UI");
    meta.extend_from_slice(&(ts.len() as u16).to_le_bytes());
    meta.extend_from_slice(&ts);
    out.extend_from_slice(&0x0002u16.to_le_bytes());
    out.extend_from_slice(&0x0000u16.to_le_bytes());
    out.extend_from_slice(b"UL");
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&(meta.len() as u32).to_le_bytes());
    out.extend_from_slice(&meta);
    // a single non-pixel element, no (7FE0,0010)
    out.extend_from_slice(&0x0028u16.to_le_bytes());
    out.extend_from_slice(&0x0010u16.to_le_bytes());
    out.extend_from_slice(b"US");
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());

    let err = decode_dicom(&out).unwrap_err().to_string();
    assert!(err.contains("no pixel data"), "got: {err}");
}
