// Integration tests for the native JPEG and WebP encoders.

use agno::codec::jpeg;
use agno::codec::webp;

// --- JPEG Tests ---

#[test]
fn jpeg_header_validity() {
    // A small solid-red image (4x4)
    let rgb = vec![255u8, 0, 0].repeat(4 * 4);
    let data = jpeg::encode_jpeg(&rgb, 4, 4, 75).unwrap();

    // Must start with SOI marker
    assert_eq!(&data[..2], &[0xFF, 0xD8], "missing SOI marker");

    // Must end with EOI marker
    assert_eq!(&data[data.len() - 2..], &[0xFF, 0xD9], "missing EOI marker");

    // Must contain DQT marker (0xFFDB)
    assert!(
        contains_marker(&data, 0xDB),
        "missing DQT marker"
    );

    // Must contain SOF0 marker (0xFFC0)
    assert!(
        contains_marker(&data, 0xC0),
        "missing SOF0 marker"
    );

    // Must contain DHT marker (0xFFC4)
    assert!(
        contains_marker(&data, 0xC4),
        "missing DHT marker"
    );

    // Must contain SOS marker (0xFFDA)
    assert!(
        contains_marker(&data, 0xDA),
        "missing SOS marker"
    );
}

#[test]
fn jpeg_roundtrip_psnr() {
    // Generate a gradient test image (32x32)
    let (width, height) = (32, 32);
    let mut rgb = vec![0u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 3;
            rgb[idx] = (x * 255 / (width - 1)) as u8; // R gradient
            rgb[idx + 1] = (y * 255 / (height - 1)) as u8; // G gradient
            rgb[idx + 2] = 128; // constant B
        }
    }

    let encoded = jpeg::encode_jpeg(&rgb, width as u32, height as u32, 90).unwrap();

    // Decode with native decoder
    let (decoded_rgb, w, h) = jpeg::decode_jpeg(&encoded).unwrap();
    assert_eq!(w, width as u32);
    assert_eq!(h, height as u32);

    // Compute PSNR
    let psnr = compute_psnr(&rgb, &decoded_rgb, width * height);
    assert!(
        psnr > 15.0,
        "PSNR {:.1}dB is too low for quality 90", psnr,
    );
}

#[test]
fn jpeg_quality_affects_size() {
    let rgb = vec![128u8; 64 * 64 * 3]; // 64x64 grey

    let low = jpeg::encode_jpeg(&rgb, 64, 64, 10).unwrap();
    let high = jpeg::encode_jpeg(&rgb, 64, 64, 95).unwrap();

    // Higher quality should produce larger output (with reasonable image)
    // For a uniform image they may be similar, but high quality should never be smaller
    // Use a more varied image for a definitive test
    let mut varied = vec![0u8; 64 * 64 * 3];
    for (i, v) in varied.iter_mut().enumerate() {
        *v = ((i * 37 + 13) % 256) as u8;
    }

    let low_v = jpeg::encode_jpeg(&varied, 64, 64, 10).unwrap();
    let high_v = jpeg::encode_jpeg(&varied, 64, 64, 95).unwrap();
    assert!(
        high_v.len() > low_v.len(),
        "quality 95 ({} bytes) should produce larger output than quality 10 ({} bytes)",
        high_v.len(),
        low_v.len(),
    );
}

#[test]
fn jpeg_1x1_pixel() {
    let rgb = vec![255u8, 0, 0];
    let data = jpeg::encode_jpeg(&rgb, 1, 1, 75).unwrap();
    assert_eq!(&data[..2], &[0xFF, 0xD8]);
    assert_eq!(&data[data.len() - 2..], &[0xFF, 0xD9]);
}

#[test]
fn jpeg_non_multiple_of_8_dimensions() {
    // 13x7 - not a multiple of 8 or 16
    let rgb = vec![100u8; 13 * 7 * 3];
    let data = jpeg::encode_jpeg(&rgb, 13, 7, 75).unwrap();
    assert_eq!(&data[..2], &[0xFF, 0xD8]);
    assert_eq!(&data[data.len() - 2..], &[0xFF, 0xD9]);

    // Verify it decodes correctly with native decoder
    let (_, w, h) = jpeg::decode_jpeg(&data).unwrap();
    assert_eq!(w, 13);
    assert_eq!(h, 7);
}

// --- WebP Tests ---

#[test]
fn webp_riff_container_validity() {
    let rgb = vec![0u8, 128, 255].repeat(4 * 4);
    let data = webp::encode_webp(&rgb, 4, 4, 75).unwrap();

    // Must start with "RIFF"
    assert_eq!(&data[..4], b"RIFF", "missing RIFF header");

    // Must contain "WEBP" at offset 8
    assert_eq!(&data[8..12], b"WEBP", "missing WEBP fourCC");

    // Must contain "VP8 " chunk
    assert_eq!(&data[12..16], b"VP8 ", "missing VP8 chunk header");
}

#[test]
fn webp_1x1_pixel() {
    let rgb = vec![0u8, 255, 0];
    let data = webp::encode_webp(&rgb, 1, 1, 75).unwrap();
    assert_eq!(&data[..4], b"RIFF");
    assert_eq!(&data[8..12], b"WEBP");
}

#[test]
fn webp_non_multiple_of_16_dimensions() {
    // 13x7
    let rgb = vec![100u8; 13 * 7 * 3];
    let data = webp::encode_webp(&rgb, 13, 7, 75).unwrap();
    assert_eq!(&data[..4], b"RIFF");
    assert_eq!(&data[8..12], b"WEBP");
}

#[test]
fn webp_riff_file_size() {
    let rgb = vec![128u8; 16 * 16 * 3];
    let data = webp::encode_webp(&rgb, 16, 16, 75).unwrap();

    // RIFF file size field (at offset 4) should equal total size - 8
    let file_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    assert_eq!(
        file_size as usize,
        data.len() - 8,
        "RIFF file size field mismatch"
    );
}

// --- JPEG Decoder Tests ---

#[test]
fn jpeg_decode_roundtrip_own_encoder() {
    let (width, height) = (64usize, 48usize);
    let mut rgb = vec![0u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 3;
            rgb[idx] = ((x * 4) % 256) as u8;
            rgb[idx + 1] = ((y * 5 + 30) % 256) as u8;
            rgb[idx + 2] = (((x + y) * 3) % 256) as u8;
        }
    }

    let jpeg_data = jpeg::encode_jpeg(&rgb, width as u32, height as u32, 85).unwrap();
    let (decoded, w, h) = jpeg::decode_jpeg(&jpeg_data).unwrap();

    assert_eq!(w, width as u32);
    assert_eq!(h, height as u32);

    let psnr = compute_psnr(&rgb, &decoded, width * height);
    assert!(
        psnr > 15.0,
        "PSNR {:.1}dB is too low for quality 85 roundtrip",
        psnr,
    );
}

#[test]
fn jpeg_decode_image_crate_encoded() {
    use image::{ImageEncoder, codecs::jpeg::JpegEncoder};

    let (width, height) = (32u32, 32u32);
    let rgb: Vec<u8> = (0..(width * height * 3)).map(|i| (i % 256) as u8).collect();

    let mut jpeg_buf = Vec::new();
    let enc = JpegEncoder::new_with_quality(&mut jpeg_buf, 80);
    enc.write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)
        .unwrap();

    let (decoded, w, h) = jpeg::decode_jpeg(&jpeg_buf).unwrap();
    assert_eq!(w, width);
    assert_eq!(h, height);
    assert_eq!(decoded.len(), (width * height * 3) as usize);
}

#[test]
fn jpeg_decode_camera_photo() {
    // Real-world camera JPEG (4032x3024, EXIF orientation 6)
    let data = std::fs::read("../tests/data/sideways.jpeg").unwrap();
    let (rgb, w, h) = jpeg::decode_jpeg(&data).unwrap();

    assert_eq!(w, 4032, "expected physical width 4032");
    assert_eq!(h, 3024, "expected physical height 3024");
    assert_eq!(rgb.len(), (4032 * 3024 * 3) as usize);

    // Spot-check: no pixel should be wildly wrong (all zeros or all 255 would indicate failure)
    let mean: f64 = rgb.iter().map(|&b| b as f64).sum::<f64>() / rgb.len() as f64;
    assert!(
        mean > 20.0 && mean < 240.0,
        "mean pixel value {:.1} suggests decode failure",
        mean
    );
}

/// Quality 100 with high-contrast edges must produce a valid JPEG decodable by
/// standard decoders. The AAN DCT can produce AC coefficients >= 1024 (category 11)
/// which exceed the baseline JPEG Huffman table limit (category 10, max 1023).
/// Encoder must clamp to avoid corrupting the bitstream.
#[test]
fn jpeg_quality_100_high_contrast_standard_decoder() {
    use image::ImageReader;
    use std::io::Cursor;

    // Create a 64x64 image with sharp edges: 8px-wide B&W stripes + color gradient.
    // This exercises high-frequency DCT content that can overflow at quality 100,
    // while being more representative than single-pixel alternation.
    let (width, height) = (64usize, 64usize);
    let mut rgb = vec![0u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 3;
            let stripe = if (x / 4) % 2 == 0 { 255u8 } else { 0u8 };
            rgb[idx] = stripe;
            rgb[idx + 1] = (y * 255 / (height - 1)) as u8;
            rgb[idx + 2] = (x * 255 / (width - 1)) as u8;
        }
    }

    let encoded = jpeg::encode_jpeg(&rgb, width as u32, height as u32, 100).unwrap();

    // Decode with the `image` crate (standard decoder) — must not crash or produce garbage
    let decoded_img = ImageReader::new(Cursor::new(&encoded))
        .with_guessed_format()
        .unwrap()
        .decode()
        .expect("standard JPEG decoder must be able to decode quality 100 output");
    let decoded_rgb = decoded_img.to_rgb8().into_raw();

    assert_eq!(decoded_rgb.len(), width * height * 3);

    let psnr = compute_psnr(&rgb, &decoded_rgb, width * height);
    assert!(
        psnr > 20.0,
        "PSNR {psnr:.1}dB is too low for quality 100 — bitstream likely corrupt"
    );
}

/// Our encoder's output must be decodable by standard JPEG decoders with correct
/// spatial frequency placement (zigzag ordering).
#[test]
fn jpeg_zigzag_correctness_standard_decoder() {
    use image::ImageReader;
    use std::io::Cursor;

    // Diagonal gradient — exercises multiple spatial frequencies
    let (width, height) = (64usize, 64usize);
    let mut rgb = vec![0u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 3;
            let val = ((x + y) * 255 / (width + height - 2)) as u8;
            rgb[idx] = val;
            rgb[idx + 1] = val;
            rgb[idx + 2] = val;
        }
    }

    let encoded = jpeg::encode_jpeg(&rgb, width as u32, height as u32, 85).unwrap();

    let decoded_img = ImageReader::new(Cursor::new(&encoded))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap();
    let decoded_rgb = decoded_img.to_rgb8().into_raw();

    let psnr = compute_psnr(&rgb, &decoded_rgb, width * height);
    assert!(
        psnr > 25.0,
        "PSNR {psnr:.1}dB is too low — zigzag ordering may be wrong"
    );
}

// --- WebP roundtrip test ---

#[test]
fn webp_roundtrip_psnr_gradient() {
    // Generate a gradient test image (32x32) — same as JPEG roundtrip test
    let (width, height) = (32, 32);
    let mut rgb = vec![0u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 3;
            rgb[idx] = (x * 255 / (width - 1)) as u8;
            rgb[idx + 1] = (y * 255 / (height - 1)) as u8;
            rgb[idx + 2] = 128;
        }
    }

    let encoded = webp::encode_webp(&rgb, width as u32, height as u32, 90).unwrap();
    let (decoded_rgb, w, h) = webp::decode_webp(&encoded).unwrap();
    assert_eq!(w, width as u32);
    assert_eq!(h, height as u32);

    let psnr = compute_psnr(&rgb, &decoded_rgb, width * height);
    eprintln!("WebP roundtrip PSNR (32x32 gradient, q90): {:.1} dB", psnr);
    assert!(
        psnr > 15.0,
        "WebP roundtrip PSNR {:.1}dB is too low for quality 90",
        psnr,
    );
}

#[test]
fn webp_roundtrip_psnr_camera_photo() {
    // Real-world camera JPEG — load, encode as WebP, decode, check PSNR
    let jpeg_data = std::fs::read("../tests/data/sideways.jpeg").unwrap();
    let (rgb, w, h) = jpeg::decode_jpeg(&jpeg_data).unwrap();
    eprintln!("Loaded JPEG: {}x{}", w, h);

    let encoded = webp::encode_webp(&rgb, w, h, 90).unwrap();
    eprintln!("WebP encoded: {} bytes", encoded.len());

    let (decoded_rgb, dw, dh) = webp::decode_webp(&encoded).unwrap();
    assert_eq!(dw, w);
    assert_eq!(dh, h);

    let psnr = compute_psnr(&rgb, &decoded_rgb, (w * h) as usize);
    eprintln!("WebP roundtrip PSNR ({}x{} camera photo, q90): {:.1} dB", w, h, psnr);
    assert!(
        psnr > 5.0,
        "WebP roundtrip PSNR {:.1}dB is too low for a camera photo at quality 90",
        psnr,
    );
}

#[test]
fn webp_cpu_roundtrip_psnr_camera_photo() {
    let jpeg_data = std::fs::read("../tests/data/sideways.jpeg").unwrap();
    let (rgb, w, h) = jpeg::decode_jpeg(&jpeg_data).unwrap();
    eprintln!("Loaded JPEG: {}x{}", w, h);

    // Force CPU encoding
    let encoded = webp::encode::encode_webp_cpu(&rgb, w, h, 90).unwrap();
    let (decoded_rgb, dw, dh) = webp::decode_webp(&encoded).unwrap();
    assert_eq!(dw, w);
    assert_eq!(dh, h);

    let psnr = compute_psnr(&rgb, &decoded_rgb, (w * h) as usize);
    eprintln!("CPU WebP roundtrip PSNR ({}x{} camera photo, q90): {:.1} dB", w, h, psnr);
    assert!(
        psnr > 25.0,
        "CPU WebP PSNR {:.1}dB is too low for a camera photo at quality 90",
        psnr,
    );
}

#[test]
fn webp_cpu_roundtrip_psnr() {
    // Force CPU path (no GPU) on a gradient image
    let (width, height) = (64, 48);
    let mut rgb = vec![0u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) * 3;
            rgb[idx] = ((x * 4) % 256) as u8;
            rgb[idx + 1] = ((y * 5 + 30) % 256) as u8;
            rgb[idx + 2] = (((x + y) * 3) % 256) as u8;
        }
    }

    // Explicitly use CPU encoder
    let encoded = webp::encode::encode_webp_cpu(&rgb, width as u32, height as u32, 90).unwrap();
    eprintln!("CPU WebP encoded: {} bytes", encoded.len());

    // Decode with our decoder
    let (decoded_rgb, w, h) = webp::decode_webp(&encoded).unwrap();
    assert_eq!(w, width as u32);
    assert_eq!(h, height as u32);

    let psnr = compute_psnr(&rgb, &decoded_rgb, width * height);
    eprintln!("CPU WebP roundtrip PSNR (own decoder): {:.1} dB", psnr);

    // Decode with image crate (standard decoder)
    use image::ImageReader;
    use std::io::Cursor;
    let img = ImageReader::new(Cursor::new(&encoded))
        .with_guessed_format()
        .unwrap()
        .decode()
        .expect("standard WebP decoder should be able to decode");
    let std_decoded = img.to_rgb8().into_raw();

    let std_psnr = compute_psnr(&rgb, &std_decoded, width * height);
    eprintln!("CPU WebP roundtrip PSNR (image crate): {:.1} dB", std_psnr);

    assert!(
        std_psnr > 15.0,
        "CPU WebP PSNR {:.1}dB is too low for quality 90 (standard decoder)",
        std_psnr,
    );

    // Verify our decoder and the standard decoder produce similar results
    let cross_psnr = compute_psnr(&decoded_rgb, &std_decoded, width * height);
    eprintln!("Cross-decoder PSNR (ours vs standard): {:.1} dB", cross_psnr);
    assert!(
        cross_psnr > 25.0,
        "Cross-decoder PSNR {:.1}dB is too low — decoders disagree",
        cross_psnr,
    );
}

// --- Progressive JPEG Tests ---

#[test]
fn jpeg_decode_progressive() {
    // Progressive JPEG (SOF2, 960x540, 12 scans)
    let data = std::fs::read("../tests/data/progressive.jpeg").unwrap();
    let (rgb, w, h) = jpeg::decode_jpeg(&data).unwrap();

    assert_eq!(w, 960, "expected width 960");
    assert_eq!(h, 540, "expected height 540");
    assert_eq!(rgb.len(), (960 * 540 * 3) as usize);

    // Compare with image crate as reference decoder
    use image::ImageReader;
    use std::io::Cursor;
    let ref_img = ImageReader::new(Cursor::new(&data))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap();
    let ref_rgb = ref_img.to_rgb8().into_raw();

    let psnr = compute_psnr(&rgb, &ref_rgb, (w * h) as usize);
    eprintln!("Progressive JPEG PSNR vs reference: {:.1} dB", psnr);
    assert!(
        psnr > 40.0,
        "Progressive JPEG PSNR vs reference {:.1}dB is too low",
        psnr,
    );
}

// --- Helpers ---

fn contains_marker(data: &[u8], marker_byte: u8) -> bool {
    data.windows(2)
        .any(|w| w[0] == 0xFF && w[1] == marker_byte)
}

fn compute_psnr(original: &[u8], decoded: &[u8], num_pixels: usize) -> f64 {
    let total_samples = num_pixels * 3;
    let mse: f64 = original
        .iter()
        .zip(decoded.iter())
        .take(total_samples)
        .map(|(&a, &b)| {
            let diff = a as f64 - b as f64;
            diff * diff
        })
        .sum::<f64>()
        / total_samples as f64;

    if mse < 0.001 {
        return 100.0; // essentially identical
    }
    10.0 * (255.0_f64 * 255.0 / mse).log10()
}
