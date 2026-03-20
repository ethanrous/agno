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
