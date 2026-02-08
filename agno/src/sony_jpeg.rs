use std::io::Write;

use crate::sony_decoder::DecodeError;

#[allow(dead_code)]
pub fn write_webp_from_rgb8_writer<W: Write>(
    writer: &mut W,
    rgb: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<(), DecodeError> {
    let enc = webp::Encoder::new(rgb, webp::PixelLayout::Rgb, width, height);
    let encoded = enc.encode(quality as f32);
    writer.write_all(&encoded)?;

    Ok(())
}
