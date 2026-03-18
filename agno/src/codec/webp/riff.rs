/// Wrap a raw VP8 bitstream in a RIFF/WebP container.
///
/// Produces a valid .webp file that any WebP decoder can read.
pub fn wrap_riff_webp(vp8_data: &[u8]) -> Vec<u8> {
    let chunk_size = vp8_data.len() as u32;
    // RIFF file size = "WEBP" (4) + VP8 chunk header (8) + VP8 data + optional pad
    let file_size = 4 + 8 + chunk_size;
    let padded_file_size = if chunk_size % 2 == 1 {
        file_size + 1
    } else {
        file_size
    };

    let mut out = Vec::with_capacity(8 + padded_file_size as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&padded_file_size.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(b"VP8 ");
    out.extend_from_slice(&chunk_size.to_le_bytes());
    out.extend_from_slice(vp8_data);
    // RIFF chunks are padded to even length.
    if chunk_size % 2 == 1 {
        out.push(0x00);
    }
    out
}
