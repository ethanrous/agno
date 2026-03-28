use std::io::Write;

use crate::sony_decoder::DecodeError;

#[cfg(feature = "webp")]
#[allow(dead_code)]
pub fn write_webp_from_rgb8_writer<W: Write>(
    writer: &mut W,
    rgb: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<(), DecodeError> {
    let encoded = crate::codec::webp::encode_webp(rgb, width, height, quality)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    writer.write_all(&encoded)?;

    Ok(())
}
