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
    assert!(contains_marker(&data, 0xDB), "missing DQT marker");

    // Must contain SOF0 marker (0xFFC0)
    assert!(contains_marker(&data, 0xC0), "missing SOF0 marker");

    // Must contain DHT marker (0xFFC4)
    assert!(contains_marker(&data, 0xC4), "missing DHT marker");

    // Must contain SOS marker (0xFFDA)
    assert!(contains_marker(&data, 0xDA), "missing SOS marker");
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
    assert!(psnr > 15.0, "PSNR {:.1}dB is too low for quality 90", psnr,);
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
fn webp_odd_dimensions_large() {
    // Odd dimensions >= 256 trigger the GPU path (if available).
    // The GPU path must use ceil(w/2) / ceil(h/2) for chroma plane dimensions,
    // matching the macroblock grid expectations in encode_webp_from_yuv.
    for (w, h) in [(257u32, 256u32), (256u32, 257u32), (513u32, 513u32)] {
        let rgb = vec![128u8; (w * h * 3) as usize];
        let result = webp::encode_webp(&rgb, w, h, 80);
        assert!(result.is_ok(), "WebP encode panicked for {}x{}", w, h);
        let data = result.unwrap();
        assert_eq!(&data[..4], b"RIFF", "Invalid RIFF header for {}x{}", w, h);
    }
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
    eprintln!(
        "WebP roundtrip PSNR ({}x{} camera photo, q90): {:.1} dB",
        w, h, psnr
    );
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
    eprintln!(
        "CPU WebP roundtrip PSNR ({}x{} camera photo, q90): {:.1} dB",
        w, h, psnr
    );
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
    eprintln!(
        "Cross-decoder PSNR (ours vs standard): {:.1} dB",
        cross_psnr
    );
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
    data.windows(2).any(|w| w[0] == 0xFF && w[1] == marker_byte)
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

// --- PDF Tests ---

#[cfg(feature = "pdf")]
mod pdf_tests {
    use std::io::Write;

    /// Generates a minimal PDF with a colored rectangle for testing.
    fn make_test_pdf_with_color(r: f32, g: f32, b: f32) -> Vec<u8> {
        let content = format!("{r} {g} {b} rg 0 0 200 100 re f");
        let content_bytes = content.as_bytes();
        let content_len = content_bytes.len();

        let mut pdf = Vec::new();
        write!(pdf, "%PDF-1.4\n").unwrap();

        let obj1_offset = pdf.len();
        write!(pdf, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n").unwrap();

        let obj2_offset = pdf.len();
        write!(
            pdf,
            "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n"
        )
        .unwrap();

        let obj3_offset = pdf.len();
        write!(
            pdf,
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>\nendobj\n"
        ).unwrap();

        let obj4_offset = pdf.len();
        write!(pdf, "4 0 obj\n<< /Length {} >>\nstream\n", content_len).unwrap();
        pdf.extend_from_slice(content_bytes);
        write!(pdf, "\nendstream\nendobj\n").unwrap();

        let xref_offset = pdf.len();
        write!(pdf, "xref\n0 5\n").unwrap();
        write!(pdf, "0000000000 65535 f \n").unwrap();
        write!(pdf, "{:010} 00000 n \n", obj1_offset).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj2_offset).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj3_offset).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj4_offset).unwrap();

        write!(pdf, "trailer\n<< /Size 5 /Root 1 0 R >>\n").unwrap();
        write!(pdf, "startxref\n{}\n%%EOF\n", xref_offset).unwrap();

        pdf
    }

    #[test]
    fn pdf_load_via_auto_detect_and_encode_to_jpeg() {
        let pdf_data = make_test_pdf_with_color(0.0, 0.0, 1.0); // Blue rect
        let dir = std::env::temp_dir();
        let path = dir.join("agno_pdf_jpeg_test.pdf");
        std::fs::write(&path, &pdf_data).unwrap();

        // Goes through detect_image_type() → load_pdf() → AgnoImage
        let img =
            agno::agno_image::load::load_agno_image_from_file(path.to_str().unwrap()).unwrap();
        assert!(img.width > 0 && img.height > 0);

        // Encode to JPEG — proves the full PDF→AgnoImage→JPEG pipeline works
        #[cfg(feature = "jpeg")]
        {
            let jpeg_bytes = img.to_jpeg(90).unwrap();
            assert!(jpeg_bytes.len() > 100, "JPEG output should be non-trivial");
            assert_eq!(
                &jpeg_bytes[0..2],
                &[0xFF, 0xD8],
                "Should start with JPEG SOI"
            );
        }

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn pdf_pixel_color_accuracy() {
        let pdf_data = make_test_pdf_with_color(0.0, 1.0, 0.0); // Green rect
        let dir = std::env::temp_dir();
        let path = dir.join("agno_pdf_color_test.pdf");
        std::fs::write(&path, &pdf_data).unwrap();

        let img =
            agno::agno_image::load::load_agno_image_from_file(path.to_str().unwrap()).unwrap();

        // Verify the AgnoImage data is valid RGB8
        let data = img.as_slice();
        assert_eq!(data.len(), (img.width * img.height * 3) as usize);

        // Sample center pixel — should be greenish
        let cx = img.width as usize / 2;
        let cy = img.height as usize / 2;
        let idx = (cy * img.width as usize + cx) * 3;
        assert!(
            data[idx] < 50,
            "Red channel should be low, got {}",
            data[idx]
        );
        assert!(
            data[idx + 1] > 200,
            "Green channel should be high, got {}",
            data[idx + 1]
        );
        assert!(
            data[idx + 2] < 50,
            "Blue channel should be low, got {}",
            data[idx + 2]
        );

        std::fs::remove_file(&path).ok();
    }
}

// --- PDF annotation tests ---

#[test]
#[cfg(feature = "pdf")]
fn pdf_annotation_renders() {
    let pdf = make_pdf_with_annotation();
    let (rgb, w, _h, _) = agno::codec::pdf::render_pdf_page(&pdf, 0, 1.0).unwrap();

    // The annotation rect is at [60 30 90 70] on a 100x100 page.
    // Center of annotation in PDF coords: (75, 50). Pixel Y = 100 - 50 = 50.
    let px = 75usize;
    let py = 50usize;
    let idx = (py * w as usize + px) * 3;
    assert!(
        rgb[idx] < 50,
        "Red should be low at annotation, got {}",
        rgb[idx]
    );
    assert!(
        rgb[idx + 1] < 50,
        "Green should be low at annotation, got {}",
        rgb[idx + 1]
    );
    assert!(
        rgb[idx + 2] > 200,
        "Blue should be high at annotation, got {}",
        rgb[idx + 2]
    );
}

#[cfg(feature = "pdf")]
fn make_pdf_with_annotation() -> Vec<u8> {
    use std::io::Write;
    let mut pdf = Vec::new();
    write!(pdf, "%PDF-1.4\n").unwrap();
    let o1 = pdf.len();
    write!(pdf, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n").unwrap();
    let o2 = pdf.len();
    write!(
        pdf,
        "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n"
    )
    .unwrap();
    let o3 = pdf.len();
    write!(pdf, "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Annots [5 0 R] >>\nendobj\n").unwrap();
    let content = b"1 0 0 rg 0 0 100 100 re f";
    let o4 = pdf.len();
    write!(pdf, "4 0 obj\n<< /Length {} >>\nstream\n", content.len()).unwrap();
    pdf.extend_from_slice(content);
    write!(pdf, "\nendstream\nendobj\n").unwrap();
    let o5 = pdf.len();
    write!(pdf, "5 0 obj\n<< /Type /Annot /Subtype /Widget /Rect [60 30 90 70] /AP << /N 6 0 R >> >>\nendobj\n").unwrap();
    let ap_content = b"0 0 1 rg 0 0 30 40 re f";
    let o6 = pdf.len();
    write!(
        pdf,
        "6 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 30 40] /Length {} >>\nstream\n",
        ap_content.len()
    )
    .unwrap();
    pdf.extend_from_slice(ap_content);
    write!(pdf, "\nendstream\nendobj\n").unwrap();
    let xref = pdf.len();
    write!(pdf, "xref\n0 7\n").unwrap();
    write!(pdf, "0000000000 65535 f \n").unwrap();
    for off in [o1, o2, o3, o4, o5, o6] {
        write!(pdf, "{:010} 00000 n \n", off).unwrap();
    }
    write!(
        pdf,
        "trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    )
    .unwrap();
    pdf
}

#[test]
#[cfg(feature = "pdf")]
fn pdf_checkbox_annotation_renders_checked_state() {
    let pdf = make_pdf_with_checkbox();
    let (rgb, w, _h, _) = agno::codec::pdf::render_pdf_page(&pdf, 0, 1.0).unwrap();

    // Checked checkbox at [0 0 30 30] should render green (from /Yes appearance).
    // PDF coords center: (15, 15). Pixel Y = 100 - 15 = 85.
    let px = 15usize;
    let py = 85usize;
    let idx = (py * w as usize + px) * 3;
    assert!(
        rgb[idx] < 50,
        "Red should be low for checked checkbox, got {}",
        rgb[idx]
    );
    assert!(
        rgb[idx + 1] > 200,
        "Green should be high for checked checkbox, got {}",
        rgb[idx + 1]
    );
    assert!(
        rgb[idx + 2] < 50,
        "Blue should be low for checked checkbox, got {}",
        rgb[idx + 2]
    );
}

#[cfg(feature = "pdf")]
fn make_pdf_with_checkbox() -> Vec<u8> {
    use std::io::Write;
    let mut pdf = Vec::new();
    write!(pdf, "%PDF-1.4\n").unwrap();
    let o1 = pdf.len();
    write!(pdf, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n").unwrap();
    let o2 = pdf.len();
    write!(
        pdf,
        "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n"
    )
    .unwrap();
    let o3 = pdf.len();
    write!(pdf, "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Annots [5 0 R] >>\nendobj\n").unwrap();
    let o4 = pdf.len();
    write!(
        pdf,
        "4 0 obj\n<< /Length 0 >>\nstream\n\nendstream\nendobj\n"
    )
    .unwrap();
    let o5 = pdf.len();
    write!(pdf, "5 0 obj\n<< /Type /Annot /Subtype /Widget /FT /Btn /Rect [0 0 30 30] /AS /Yes /AP << /N << /Yes 6 0 R /Off 7 0 R >> >> >>\nendobj\n").unwrap();
    let yes_ap = b"0 1 0 rg 0 0 30 30 re f";
    let o6 = pdf.len();
    write!(
        pdf,
        "6 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 30 30] /Length {} >>\nstream\n",
        yes_ap.len()
    )
    .unwrap();
    pdf.extend_from_slice(yes_ap);
    write!(pdf, "\nendstream\nendobj\n").unwrap();
    let off_ap = b"1 1 1 rg 0 0 30 30 re f";
    let o7 = pdf.len();
    write!(
        pdf,
        "7 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 30 30] /Length {} >>\nstream\n",
        off_ap.len()
    )
    .unwrap();
    pdf.extend_from_slice(off_ap);
    write!(pdf, "\nendstream\nendobj\n").unwrap();
    let xref = pdf.len();
    write!(pdf, "xref\n0 8\n").unwrap();
    write!(pdf, "0000000000 65535 f \n").unwrap();
    for off in [o1, o2, o3, o4, o5, o6, o7] {
        write!(pdf, "{:010} 00000 n \n", off).unwrap();
    }
    write!(
        pdf,
        "trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    )
    .unwrap();
    pdf
}

// --- PNG Tests ---

#[cfg(feature = "png")]
#[test]
fn png_rgb_roundtrip_exact() {
    use agno::codec::png::{decode_png, encode_png};
    let src: Vec<u8> = vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120];
    let encoded = encode_png(&src, 2, 2, 3).expect("encode");
    let (decoded, w, h, channels) = decode_png(&encoded).expect("decode");
    assert_eq!((w, h, channels), (2, 2, 3));
    assert_eq!(decoded, src, "PNG is lossless for RGB");
}

#[cfg(feature = "png")]
#[test]
fn png_rgba_roundtrip_exact() {
    use agno::codec::png::{decode_png, encode_png};
    let src: Vec<u8> = vec![
        10, 20, 30, 255, 40, 50, 60, 128, 70, 80, 90, 64, 100, 110, 120, 0,
    ];
    let encoded = encode_png(&src, 2, 2, 4).expect("encode");
    let (decoded, w, h, channels) = decode_png(&encoded).expect("decode");
    assert_eq!((w, h, channels), (2, 2, 4));
    assert_eq!(decoded, src, "PNG is lossless for RGBA");
}

#[cfg(feature = "png")]
#[test]
fn png_encoder_rejects_bad_channels() {
    use agno::codec::png::encode_png;
    let src = vec![0u8; 4];
    assert!(
        encode_png(&src, 1, 1, 2).is_err(),
        "channels=2 not supported"
    );
    assert!(
        encode_png(&src, 1, 1, 5).is_err(),
        "channels=5 not supported"
    );
}
