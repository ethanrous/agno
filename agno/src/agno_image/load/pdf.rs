use std::error::Error;

use crate::{agno_image::AgnoImage, exif::ExifContext};

/// Load a specific page from PDF bytes, with optional max dimensions.
///
/// `page_index` is 0-based. If `max_dims` is Some((w, h)), the page is scaled
/// to fit within those dimensions (preserving aspect ratio).
#[cfg(feature = "pdf")]
pub fn load_pdf_page_from_bytes(
    data: &[u8],
    page_index: usize,
    max_dims: Option<(u32, u32)>,
    exif: ExifContext,
) -> Result<AgnoImage, Box<dyn Error>> {
    let doc = crate::codec::pdf::document::PdfDocument::open(data)?;
    let page_count = doc.page_count();
    if page_index >= page_count {
        return Err(format!(
            "PDF page {page_index} not found (document has {page_count} pages)"
        )
        .into());
    }
    let scale = scale_for_dims_from_doc(&doc, page_index, max_dims);
    let (rgb, width, height) =
        crate::codec::pdf::render_pdf_page_from_doc(&doc, page_index, scale)?;

    let mut img = AgnoImage::new(rgb, width as u64, height as u64, exif);
    img.set_page_count(page_count as u64);
    Ok(img)
}

/// Compute the render scale from an already-opened document.
/// Falls back to 4.0 (288 DPI) for crisp vector rendering when no constraint is given.
#[cfg(feature = "pdf")]
fn scale_for_dims_from_doc(
    doc: &crate::codec::pdf::document::PdfDocument,
    page_index: usize,
    max_dims: Option<(u32, u32)>,
) -> f32 {
    const DEFAULT_SCALE: f32 = 4.0;
    const MAX_SCALE: f32 = 8.0;
    match max_dims {
        Some((max_w, max_h)) => {
            if let Ok((x0, y0, x1, y1)) = doc.page_media_box(page_index) {
                let page_w = (x1 - x0).abs() as f32;
                let page_h = (y1 - y0).abs() as f32;
                if page_w > 0.0 && page_h > 0.0 {
                    let sx = max_w as f32 / page_w;
                    let sy = max_h as f32 / page_h;
                    return sx.min(sy).min(MAX_SCALE);
                }
            }
            DEFAULT_SCALE
        }
        None => DEFAULT_SCALE,
    }
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
