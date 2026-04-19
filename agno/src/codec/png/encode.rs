use std::error::Error;

const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// Encode RGB (channels=3) or RGBA (channels=4) pixels to a PNG byte stream.
/// Uses filter method None on every scanline.
pub fn encode_png(
    pixels: &[u8],
    width: u32,
    height: u32,
    channels: u8,
) -> Result<Vec<u8>, Box<dyn Error>> {
    if channels != 3 && channels != 4 {
        return Err(format!("encode_png: channels must be 3 or 4, got {channels}").into());
    }
    let expected_bytes = (width as usize) * (height as usize) * (channels as usize);
    if pixels.len() != expected_bytes {
        return Err(format!(
            "encode_png: pixel buffer length {} does not match {}x{}x{} = {}",
            pixels.len(),
            width,
            height,
            channels,
            expected_bytes
        )
        .into());
    }

    let color_type: u8 = if channels == 3 { 2 } else { 6 };
    let stride = (width as usize) * (channels as usize);

    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in 0..height as usize {
        raw.push(0u8); // filter: None
        let start = row * stride;
        raw.extend_from_slice(&pixels[start..start + stride]);
    }

    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);

    let mut out = Vec::with_capacity(compressed.len() + 64);
    out.extend_from_slice(&PNG_SIGNATURE);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8);
    ihdr.push(color_type);
    ihdr.push(0); // compression = deflate
    ihdr.push(0); // filter method = adaptive (per-scanline filter byte)
    ihdr.push(0); // interlace = none
    write_chunk(&mut out, *b"IHDR", &ihdr);
    write_chunk(&mut out, *b"IDAT", &compressed);
    write_chunk(&mut out, *b"IEND", &[]);
    Ok(out)
}

fn write_chunk(out: &mut Vec<u8>, chunk_type: [u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let crc_start = out.len();
    out.extend_from_slice(&chunk_type);
    out.extend_from_slice(data);
    let crc = crc32(&out[crc_start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// PNG CRC-32 (polynomial 0xEDB88320). Precomputed table for speed.
fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, entry) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            }
            *entry = c;
        }
        t
    });
    let mut crc: u32 = 0xFFFFFFFF;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_known_value() {
        // CRC-32 of b"IEND" with no data payload is 0xAE426082.
        assert_eq!(crc32(b"IEND"), 0xAE426082);
    }

    #[test]
    fn crc32_matches_known_ihdr_prefix() {
        assert_eq!(crc32(b"IHDR"), 0xA8A1AE0A);
    }

    #[test]
    fn encode_png_rgb_produces_signature() {
        let pixels = vec![255u8, 0, 0];
        let png = encode_png(&pixels, 1, 1, 3).unwrap();
        assert_eq!(&png[..8], &PNG_SIGNATURE);
    }
}
