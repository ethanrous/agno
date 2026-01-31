use std::error::Error;

#[cfg(feature = "pdf")]
use pdfium_render::prelude::{PdfPageRenderRotation, PdfRenderConfig, Pdfium};

use crate::{agno_image::AgnoImage, exif::ExifContext};

#[cfg(feature = "pdf")]
pub fn load_pdf(path: &str, exif: ExifContext) -> Result<AgnoImage, Box<dyn Error>> {
    let pdfium = Pdfium::new(Pdfium::bind_to_statically_linked_library().unwrap());

    // Load the document from the given path...
    let document = pdfium.load_pdf_from_file(path, None)?;

    // ... set rendering options that will be applied to all pages...
    let render_config = PdfRenderConfig::new()
        .set_target_width(2000)
        .set_maximum_height(2000)
        .rotate_if_landscape(PdfPageRenderRotation::Degrees90, true);

    let first_page = document.pages().get(0)?;
    let img = first_page
        .render_with_config(&render_config)?
        .as_image()
        .to_rgb8();

    let (width, height) = img.dimensions();

    return Ok(AgnoImage::new(
        img.into_raw(),
        width as u64,
        height as u64,
        exif,
    ));
}

#[cfg(not(feature = "pdf"))]
pub fn load_pdf(path: &str, exif: ExifContext) -> Result<AgnoImage, Box<dyn Error>> {
    Err("PDF support is not enabled. Please enable the 'pdf' feature.".into())
}
