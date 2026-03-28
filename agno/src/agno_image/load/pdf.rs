use std::error::Error;
#[cfg(feature = "pdf")]
use std::sync::Arc;

use crate::{agno_image::AgnoImage, exif::ExifContext};

#[cfg(feature = "pdf")]
use hayro::{RenderSettings, hayro_interpret::InterpreterSettings, hayro_syntax::Pdf};

#[cfg(feature = "pdf-pdfium")]
use pdfium_render::prelude::{PdfPageRenderRotation, PdfRenderConfig, Pdfium};

/// Default scale factor for PDF rendering.
/// PDF points are 1/72 inch; 2x gives 144 DPI which is good for thumbnails/previews.
#[cfg(feature = "pdf")]
const DEFAULT_SCALE: f32 = 2.0;

/// Maximum scale factor to prevent excessive memory usage.
#[cfg(feature = "pdf")]
const MAX_SCALE: f32 = 4.0;

/// Default max pixel dimension for pdfium fallback rendering.
#[cfg(feature = "pdf-pdfium")]
const PDFIUM_TARGET_SIZE: i32 = 2000;

/// Load a specific page from PDF bytes, with optional max dimensions.
///
/// Tries hayro (pure Rust) first. If hayro produces a blank page and `pdf-pdfium`
/// is enabled, falls back to pdfium for robust rendering of complex/linearized PDFs.
///
/// `page_index` is 0-based. If `max_dims` is Some((w, h)), the page is scaled
/// to fit within those dimensions (preserving aspect ratio). Otherwise `DEFAULT_SCALE` is used.
#[cfg(feature = "pdf")]
pub fn load_pdf_page_from_bytes(
    data: &[u8],
    page_index: usize,
    max_dims: Option<(u32, u32)>,
    exif: ExifContext,
) -> Result<AgnoImage, Box<dyn Error>> {
    let (rgb, width, height, page_count) = match render_with_hayro(data, page_index, max_dims) {
        Ok(result) => result,
        Err(hayro_err) => {
            tracing::warn!("hayro PDF render failed: {hayro_err}");
            render_pdfium_fallback(data, page_index, max_dims, hayro_err)?
        }
    };

    let mut img = AgnoImage::new(rgb, width as u64, height as u64, exif);
    img.set_page_count(page_count as u64);
    Ok(img)
}

/// Render a PDF page using hayro (pure Rust).
/// Returns (rgb8_data, width, height, page_count) on success.
#[cfg(feature = "pdf")]
#[allow(clippy::type_complexity)]
fn render_with_hayro(
    data: &[u8],
    page_index: usize,
    max_dims: Option<(u32, u32)>,
) -> Result<(Vec<u8>, u32, u32, usize), Box<dyn Error>> {
    let pdf_data = Arc::new(data.to_vec());
    let pdf = Pdf::new(pdf_data).map_err(|e| format!("Failed to parse PDF: {e:?}"))?;

    let pages = pdf.pages();
    if pages.is_empty() {
        return Err("PDF document has no pages".into());
    }
    let page = pages.get(page_index).ok_or_else(|| {
        format!(
            "PDF page {page_index} not found (document has {} pages)",
            pages.len()
        )
    })?;

    let media_box = page.media_box();
    let page_w = (media_box.x1 - media_box.x0).abs() as f32;
    let page_h = (media_box.y1 - media_box.y0).abs() as f32;

    if page_w <= 0.0 || page_h <= 0.0 {
        return Err(format!("Invalid PDF page dimensions: {page_w}x{page_h}").into());
    }

    let scale = match max_dims {
        Some((max_w, max_h)) => {
            let sx = max_w as f32 / page_w;
            let sy = max_h as f32 / page_h;
            sx.min(sy).min(MAX_SCALE)
        }
        None => DEFAULT_SCALE,
    };

    if scale <= 0.0 {
        return Err("Render scale is zero or negative (check max_dims)".into());
    }

    let render_settings = RenderSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        ..Default::default()
    };

    let interpreter_settings = InterpreterSettings::default();
    let pixmap = hayro::render(page, &interpreter_settings, &render_settings);

    let width = u32::from(pixmap.width());
    let height = u32::from(pixmap.height());

    if width == 0 || height == 0 {
        return Err("PDF render produced a zero-dimension image".into());
    }

    // Detect blank renders — hayro silently fails on some PDFs (e.g. linearized)
    // by rendering only the white background with no page content.
    let rgba = pixmap.data_as_u8_slice();
    let all_white = rgba.chunks_exact(4).all(|c| c == [255, 255, 255, 255]);
    if all_white {
        return Err(
            "PDF page rendered as blank (hayro could not parse page content — \
             this may be a linearized PDF or use unsupported features)"
                .into(),
        );
    }

    // Convert premultiplied RGBA8 → RGB8, composited over white background.
    // For premultiplied src over white: out = premul + (255 - alpha)
    let pixel_count = width as usize * height as usize;
    let mut rgb = Vec::with_capacity(pixel_count * 3);

    for chunk in rgba.chunks_exact(4) {
        let a = chunk[3];
        let inv_a = 255 - a;
        rgb.push(chunk[0].saturating_add(inv_a));
        rgb.push(chunk[1].saturating_add(inv_a));
        rgb.push(chunk[2].saturating_add(inv_a));
    }

    Ok((rgb, width, height, pages.len()))
}

/// Pdfium fallback: renders a PDF page using Google's pdfium (C++, statically linked).
/// Only compiled when the `pdf-pdfium` feature is enabled.
#[cfg(feature = "pdf-pdfium")]
fn render_pdfium_fallback(
    data: &[u8],
    page_index: usize,
    max_dims: Option<(u32, u32)>,
    _hayro_err: Box<dyn Error>,
) -> Result<(Vec<u8>, u32, u32, usize), Box<dyn Error>> {
    tracing::info!("Falling back to pdfium for PDF rendering");

    let pdfium = Pdfium::new(Pdfium::bind_to_statically_linked_library()?);
    let document = pdfium.load_pdf_from_byte_slice(data, None)?;
    let page_count = document.pages().len() as usize;
    let page = document.pages().get(
        u16::try_from(page_index)
            .map_err(|_| format!("PDF page index {page_index} exceeds u16 range"))?,
    )?;

    let (target_w, target_h) = max_dims
        .map(|(w, h)| (w as i32, h as i32))
        .unwrap_or((PDFIUM_TARGET_SIZE, PDFIUM_TARGET_SIZE));

    let render_config = PdfRenderConfig::new()
        .set_target_width(target_w)
        .set_maximum_height(target_h)
        .rotate_if_landscape(PdfPageRenderRotation::Degrees90, true);

    let img = page
        .render_with_config(&render_config)?
        .as_image()
        .to_rgb8();
    let (width, height) = img.dimensions();

    Ok((img.into_raw(), width, height, page_count))
}

/// When pdfium is not available, just propagate the original hayro error.
#[cfg(all(feature = "pdf", not(feature = "pdf-pdfium")))]
#[allow(clippy::type_complexity)]
fn render_pdfium_fallback(
    _data: &[u8],
    _page_index: usize,
    _max_dims: Option<(u32, u32)>,
    hayro_err: Box<dyn Error>,
) -> Result<(Vec<u8>, u32, u32, usize), Box<dyn Error>> {
    Err(hayro_err)
}

#[cfg(not(feature = "pdf"))]
pub fn load_pdf_page_from_bytes(
    _data: &[u8],
    _page_index: usize,
    _max_dims: Option<(u32, u32)>,
    _exif: ExifContext,
) -> Result<AgnoImage, Box<dyn Error>> {
    Err("PDF support is not enabled. Please enable the 'pdf' feature.".into())
}

#[cfg(test)]
#[cfg(feature = "pdf")]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_test_pdf() -> Vec<u8> {
        make_test_pdf_with_color(1.0, 0.0, 0.0)
    }

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
        )
        .unwrap();

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
    fn load_pdf_produces_valid_image() {
        let dir = std::env::temp_dir();
        let path = dir.join("agno_test.pdf");
        std::fs::write(&path, make_test_pdf()).unwrap();

        let data = std::fs::read(&path).unwrap();
        let img = load_pdf_page_from_bytes(&data, 0, None, ExifContext::default()).unwrap();

        assert!(img.width > 0);
        assert!(img.height > 0);
        assert_eq!(img.as_slice().len(), (img.width * img.height * 3) as usize);

        let data = img.as_slice();
        let cx = img.width as usize / 2;
        let cy = img.height as usize / 2;
        let idx = (cy * img.width as usize + cx) * 3;
        assert!(
            data[idx] > 200,
            "Red channel should be high, got {}",
            data[idx]
        );
        assert!(
            data[idx + 1] < 50,
            "Green should be low, got {}",
            data[idx + 1]
        );
        assert!(
            data[idx + 2] < 50,
            "Blue should be low, got {}",
            data[idx + 2]
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn load_pdf_page_out_of_range() {
        let data = make_test_pdf();
        let result = load_pdf_page_from_bytes(&data, 99, None, ExifContext::default());
        assert!(result.is_err());
    }

    #[test]
    fn load_pdf_with_max_dims() {
        let data = make_test_pdf();
        let img =
            load_pdf_page_from_bytes(&data, 0, Some((100, 100)), ExifContext::default()).unwrap();

        assert!(
            img.width <= 100,
            "Width should be <= 100, got {}",
            img.width
        );
        assert!(
            img.height <= 100,
            "Height should be <= 100, got {}",
            img.height
        );
        assert!(img.width > 0 && img.height > 0);
    }

    #[test]
    fn load_pdf_invalid_data_returns_error() {
        let result =
            load_pdf_page_from_bytes(b"%PDF-1.4\ngarbage", 0, None, ExifContext::default());
        assert!(result.is_err());
    }

    fn make_multi_page_pdf() -> Vec<u8> {
        let pages: &[(f32, f32, f32)] = &[
            (1.0, 0.0, 0.0), // red
            (0.0, 1.0, 0.0), // green
            (0.0, 0.0, 1.0), // blue
        ];

        let mut pdf = Vec::new();
        write!(pdf, "%PDF-1.4\n").unwrap();

        // Object 1: Catalog
        let obj1_offset = pdf.len();
        write!(pdf, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n").unwrap();

        // Build page kid refs and content streams
        // Pages use objects 3,5,7 and contents use 4,6,8
        let mut kids = String::from("[");
        for i in 0..pages.len() {
            let page_obj = 3 + i * 2;
            if i > 0 {
                kids.push(' ');
            }
            kids.push_str(&format!("{page_obj} 0 R"));
        }
        kids.push(']');

        // Object 2: Pages
        let obj2_offset = pdf.len();
        write!(
            pdf,
            "2 0 obj\n<< /Type /Pages /Kids {kids} /Count {} >>\nendobj\n",
            pages.len()
        )
        .unwrap();

        let mut offsets = vec![0usize; pages.len() * 2]; // page_obj, content_obj pairs

        for (i, (r, g, b)) in pages.iter().enumerate() {
            let content = format!("{r} {g} {b} rg 0 0 200 100 re f");
            let page_obj_num = 3 + i * 2;
            let content_obj_num = 4 + i * 2;

            offsets[i * 2] = pdf.len();
            write!(
                pdf,
                "{page_obj_num} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents {content_obj_num} 0 R >>\nendobj\n"
            )
            .unwrap();

            offsets[i * 2 + 1] = pdf.len();
            write!(
                pdf,
                "{content_obj_num} 0 obj\n<< /Length {} >>\nstream\n",
                content.len()
            )
            .unwrap();
            pdf.extend_from_slice(content.as_bytes());
            write!(pdf, "\nendstream\nendobj\n").unwrap();
        }

        let total_objects = 2 + pages.len() * 2; // catalog + pages + (page + content) per page
        let xref_offset = pdf.len();
        write!(pdf, "xref\n0 {}\n", total_objects + 1).unwrap();
        write!(pdf, "0000000000 65535 f \n").unwrap();
        write!(pdf, "{:010} 00000 n \n", obj1_offset).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj2_offset).unwrap();
        for off in &offsets {
            write!(pdf, "{:010} 00000 n \n", off).unwrap();
        }

        write!(
            pdf,
            "trailer\n<< /Size {} /Root 1 0 R >>\n",
            total_objects + 1
        )
        .unwrap();
        write!(pdf, "startxref\n{xref_offset}\n%%EOF\n").unwrap();

        pdf
    }

    #[test]
    fn multi_page_pdf_reports_correct_page_count() {
        let data = make_multi_page_pdf();
        let img = load_pdf_page_from_bytes(&data, 0, None, ExifContext::default()).unwrap();
        assert_eq!(
            img.page_count, 3,
            "Expected 3 pages, got {}",
            img.page_count
        );
    }

    #[test]
    fn multi_page_pdf_loads_each_page() {
        let data = make_multi_page_pdf();
        for page in 0..3 {
            let img = load_pdf_page_from_bytes(&data, page, None, ExifContext::default()).unwrap();
            assert!(
                img.width > 0 && img.height > 0,
                "Page {page} has zero dimensions"
            );
            assert_eq!(img.page_count, 3, "Page {page} should report total count 3");
        }
    }
}
