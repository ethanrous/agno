//! Integration tests for the libheif-based HEIC loader.
//! Only compiled when the `heic-c` feature is enabled.
#![cfg(feature = "heic-c")]

use std::path::Path;

fn test_data(name: &str) -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    format!("{manifest}/../tests/data/{name}")
}

#[test]
fn libheif_loads_heic_grid_image() {
    let path = test_data("sideways2.heic");
    assert!(Path::new(&path).exists(), "Test file missing: {path}");

    let img = agno::agno_image::load::load::load_agno_image_from_file(&path).unwrap();
    assert!(img.width > 0 && img.height > 0);
    let total_pixels = img.width * img.height;
    assert!(
        total_pixels > 1_000_000,
        "Image too small: {total_pixels} pixels"
    );
}

#[test]
fn libheif_loads_heic_single_tile() {
    let path = test_data("test-heic.heic");
    assert!(Path::new(&path).exists(), "Test file missing: {path}");

    let img = agno::agno_image::load::load::load_agno_image_from_file(&path).unwrap();
    assert!(img.width > 0 && img.height > 0);
}

/// End-to-end: HEIC → libheif → RGB → our JPEG encoder → standard decoder.
/// Verifies the full pipeline produces output decodable by standard decoders.
#[test]
fn libheif_heic_to_jpeg_roundtrip() {
    use image::ImageReader;
    use std::io::Cursor;

    let path = test_data("sideways2.heic");
    assert!(Path::new(&path).exists(), "Test file missing: {path}");

    let img = agno::agno_image::load::load::load_agno_image_from_file(&path).unwrap();
    let rgb = img.as_slice();
    let width = img.width as u32;
    let height = img.height as u32;

    // Encode with our JPEG encoder at quality 100
    let jpeg_data = img.to_jpeg(100).unwrap();

    // Decode with a standard JPEG decoder (image crate / libjpeg-turbo)
    let decoded_img = ImageReader::new(Cursor::new(&jpeg_data))
        .with_guessed_format()
        .unwrap()
        .decode()
        .expect("standard decoder must handle our quality-100 JPEG");
    let decoded_rgb = decoded_img.to_rgb8();

    assert_eq!(decoded_rgb.width(), width);
    assert_eq!(decoded_rgb.height(), height);

    // Compute PSNR between original RGB and decoded JPEG
    let decoded = decoded_rgb.into_raw();
    let total_samples = (width as usize) * (height as usize) * 3;
    let mse: f64 = rgb
        .iter()
        .zip(decoded.iter())
        .take(total_samples)
        .map(|(&a, &b)| {
            let d = a as f64 - b as f64;
            d * d
        })
        .sum::<f64>()
        / total_samples as f64;
    let psnr = if mse < 0.001 {
        100.0
    } else {
        10.0 * (255.0_f64 * 255.0 / mse).log10()
    };

    assert!(
        psnr > 30.0,
        "PSNR {psnr:.1}dB too low — JPEG encoder may be corrupting the bitstream"
    );
}
