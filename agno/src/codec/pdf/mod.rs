pub mod color;
pub mod content;
pub mod document;
pub mod font;
pub mod graphics;
pub mod image;
pub mod lexer;
pub mod objects;
pub mod render;
pub mod stream;
pub mod text;
pub mod xref;

use std::error::Error;

/// Render a single page from a PDF document to RGB8 pixels.
///
/// `page_index` is 0-based. `scale` controls resolution (1.0 = 72 DPI, 2.0 = 144 DPI).
/// Returns (rgb8_data, width, height, page_count).
pub fn render_pdf_page(
    data: &[u8],
    page_index: usize,
    scale: f32,
) -> Result<(Vec<u8>, u32, u32, usize), Box<dyn Error>> {
    if data.len() < 5 || &data[..5] != b"%PDF-" {
        return Err("Not a valid PDF file".into());
    }
    let doc = document::PdfDocument::open(data)?;
    let page_count = doc.page_count();

    if page_index >= page_count {
        return Err(format!(
            "PDF page {page_index} not found (document has {page_count} pages)"
        )
        .into());
    }

    let pixmap = render::render_page(&doc, page_index, scale)?;
    let width = pixmap.width();
    let height = pixmap.height();
    let rgb = render::pixmap_to_rgb8(&pixmap);

    Ok((rgb, width, height, page_count))
}

/// Get the number of pages in a PDF without rendering.
pub fn pdf_page_count(data: &[u8]) -> Result<usize, Box<dyn Error>> {
    if data.len() < 5 || &data[..5] != b"%PDF-" {
        return Err("Not a valid PDF file".into());
    }
    let doc = document::PdfDocument::open(data)?;
    Ok(doc.page_count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_test_pdf(page_count: usize) -> Vec<u8> {
        let mut pdf = Vec::new();
        write!(pdf, "%PDF-1.4\n").unwrap();
        let obj1_off = pdf.len();
        write!(pdf, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n").unwrap();

        let mut kids = String::from("[");
        for i in 0..page_count {
            if i > 0 { kids.push(' '); }
            kids.push_str(&format!("{} 0 R", 3 + i * 2));
        }
        kids.push(']');

        let obj2_off = pdf.len();
        write!(pdf, "2 0 obj\n<< /Type /Pages /Kids {kids} /Count {page_count} >>\nendobj\n").unwrap();

        let mut offsets = Vec::new();
        for i in 0..page_count {
            let page_num = 3 + i * 2;
            let content_num = 4 + i * 2;
            let content = format!("{} 0 0 rg 0 0 100 100 re f", if i % 2 == 0 { "1 0 0" } else { "0 0 1" });

            offsets.push(pdf.len());
            write!(pdf, "{page_num} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents {content_num} 0 R >>\nendobj\n").unwrap();
            offsets.push(pdf.len());
            write!(pdf, "{content_num} 0 obj\n<< /Length {} >>\nstream\n", content.len()).unwrap();
            pdf.extend_from_slice(content.as_bytes());
            write!(pdf, "\nendstream\nendobj\n").unwrap();
        }

        let total = 2 + page_count * 2;
        let xref_off = pdf.len();
        write!(pdf, "xref\n0 {}\n", total + 1).unwrap();
        write!(pdf, "0000000000 65535 f \n").unwrap();
        write!(pdf, "{:010} 00000 n \n", obj1_off).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj2_off).unwrap();
        for off in &offsets {
            write!(pdf, "{:010} 00000 n \n", off).unwrap();
        }
        write!(pdf, "trailer\n<< /Size {} /Root 1 0 R >>\n", total + 1).unwrap();
        write!(pdf, "startxref\n{xref_off}\n%%EOF\n").unwrap();
        pdf
    }

    #[test]
    fn page_count_single_page() {
        let pdf = make_test_pdf(1);
        assert_eq!(pdf_page_count(&pdf).unwrap(), 1);
    }

    #[test]
    fn page_count_multi_page() {
        let pdf = make_test_pdf(5);
        assert_eq!(pdf_page_count(&pdf).unwrap(), 5);
    }

    #[test]
    fn invalid_pdf_header() {
        assert!(pdf_page_count(b"not a pdf").is_err());
    }

    #[test]
    fn empty_pdf() {
        assert!(pdf_page_count(b"").is_err());
    }

    #[test]
    fn render_pdf_page_basic() {
        let pdf = make_test_pdf(1);
        let (rgb, w, h, pages) = render_pdf_page(&pdf, 0, 1.0).unwrap();
        assert_eq!(w, 100);
        assert_eq!(h, 100);
        assert_eq!(pages, 1);
        assert_eq!(rgb.len(), 100 * 100 * 3);

        // Center pixel should be red (first page uses "1 0 0 rg")
        let idx = (50 * 100 + 50) * 3;
        assert!(rgb[idx] > 200, "Red should be high, got {}", rgb[idx]);
        assert!(rgb[idx + 1] < 50, "Green should be low, got {}", rgb[idx + 1]);
    }

    #[test]
    fn render_pdf_page_scaled() {
        let pdf = make_test_pdf(1);
        let (_, w, h, _) = render_pdf_page(&pdf, 0, 2.0).unwrap();
        assert_eq!(w, 200);
        assert_eq!(h, 200);
    }

    #[test]
    fn render_pdf_page_out_of_range() {
        let pdf = make_test_pdf(1);
        assert!(render_pdf_page(&pdf, 5, 1.0).is_err());
    }

    #[test]
    fn render_multi_page() {
        let pdf = make_test_pdf(3);
        for page in 0..3 {
            let (rgb, w, h, count) = render_pdf_page(&pdf, page, 1.0).unwrap();
            assert_eq!(count, 3);
            assert!(w > 0 && h > 0);
            assert_eq!(rgb.len(), (w * h * 3) as usize);
        }
    }
}
