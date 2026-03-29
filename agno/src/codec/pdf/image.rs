use std::error::Error;

pub struct PdfImage {
    pub rgb_data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Decode an Image XObject from its decompressed stream data.
pub fn decode_image_xobject(
    stream_data: &[u8],
    width: u32,
    height: u32,
    bits_per_component: u8,
    color_space: &super::color::ColorSpace,
    filter: Option<&[u8]>,
) -> Result<PdfImage, Box<dyn Error>> {
    // DCTDecode = JPEG — decode using agno's JPEG codec
    if filter == Some(b"DCTDecode") {
        #[cfg(feature = "jpeg")]
        {
            let (rgb, w, h) = crate::codec::jpeg::decode_jpeg(stream_data)?;
            return Ok(PdfImage { rgb_data: rgb, width: w, height: h });
        }
        #[cfg(not(feature = "jpeg"))]
        return Err("JPEG decoding not enabled".into());
    }

    // Raw pixel data — convert to RGB8 based on color space and bpc
    let num_components = color_space.num_components();
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(3))
        .ok_or("Image dimensions overflow")?;
    let mut rgb_data = Vec::with_capacity(pixel_count);
    let max_val = ((1u32 << bits_per_component) - 1) as f64;

    match bits_per_component {
        8 => {
            for pixel in stream_data.chunks(num_components) {
                if pixel.len() < num_components { break; }
                let components: Vec<f64> = pixel.iter().map(|&b| b as f64 / max_val).collect();
                let (r, g, b) = super::color::to_rgb(color_space, &components);
                rgb_data.push((r.clamp(0.0, 1.0) * 255.0).round() as u8);
                rgb_data.push((g.clamp(0.0, 1.0) * 255.0).round() as u8);
                rgb_data.push((b.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
        16 => {
            for pixel in stream_data.chunks(num_components * 2) {
                if pixel.len() < num_components * 2 { break; }
                let components: Vec<f64> = pixel.chunks(2)
                    .take(num_components)
                    .map(|b| u16::from_be_bytes([b[0], b[1]]) as f64 / max_val)
                    .collect();
                let (r, g, b) = super::color::to_rgb(color_space, &components);
                rgb_data.push((r.clamp(0.0, 1.0) * 255.0).round() as u8);
                rgb_data.push((g.clamp(0.0, 1.0) * 255.0).round() as u8);
                rgb_data.push((b.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
        1 | 2 | 4 => {
            // Sub-byte packing, row-padded to byte boundary
            let mask = (1u8 << bits_per_component) - 1;
            let row_bits = width as usize * num_components * bits_per_component as usize;
            let row_bytes = (row_bits + 7) / 8;

            for row in 0..height as usize {
                let row_start = row * row_bytes;
                let mut bit_offset = 0usize;
                for _col in 0..width as usize {
                    let mut components = Vec::with_capacity(num_components);
                    for _ in 0..num_components {
                        let byte_idx = row_start + bit_offset / 8;
                        let bit_shift = 8 - bits_per_component as usize - (bit_offset % 8);
                        let val = if byte_idx < stream_data.len() {
                            (stream_data[byte_idx] >> bit_shift) & mask
                        } else { 0 };
                        components.push(val as f64 / max_val);
                        bit_offset += bits_per_component as usize;
                    }
                    let (r, g, b) = super::color::to_rgb(color_space, &components);
                    rgb_data.push((r.clamp(0.0, 1.0) * 255.0).round() as u8);
                    rgb_data.push((g.clamp(0.0, 1.0) * 255.0).round() as u8);
                    rgb_data.push((b.clamp(0.0, 1.0) * 255.0).round() as u8);
                }
            }
        }
        _ => return Err(format!("Unsupported bits per component: {bits_per_component}").into()),
    }

    Ok(PdfImage { rgb_data, width, height })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::color::ColorSpace;

    #[test]
    fn decode_raw_rgb_image() {
        let data = vec![
            255, 0, 0,    0, 255, 0,
            0, 0, 255,    255, 255, 0,
        ];
        let img = decode_image_xobject(&data, 2, 2, 8, &ColorSpace::DeviceRGB, None).unwrap();
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        assert_eq!(img.rgb_data.len(), 12);
        assert_eq!(&img.rgb_data[0..3], &[255, 0, 0]);
    }

    #[test]
    fn decode_gray_image() {
        let data = vec![0, 128, 255, 64];
        let img = decode_image_xobject(&data, 2, 2, 8, &ColorSpace::DeviceGray, None).unwrap();
        assert_eq!(img.rgb_data.len(), 12);
        assert_eq!(&img.rgb_data[0..3], &[0, 0, 0]);
        assert_eq!(&img.rgb_data[3..6], &[128, 128, 128]);
    }

    #[test]
    fn decode_cmyk_image() {
        let data = vec![255, 0, 0, 0]; // pure cyan
        let img = decode_image_xobject(&data, 1, 1, 8, &ColorSpace::DeviceCMYK, None).unwrap();
        assert!(img.rgb_data[0] < 10);
        assert!(img.rgb_data[1] > 240);
        assert!(img.rgb_data[2] > 240);
    }

    #[test]
    fn decode_1bpc_gray() {
        // 1-bit per component, 4 pixels: 0b1010_0000 = black, white, black, white
        let data = vec![0b1010_0000];
        let img = decode_image_xobject(&data, 4, 1, 1, &ColorSpace::DeviceGray, None).unwrap();
        assert_eq!(img.rgb_data.len(), 12);
        assert_eq!(&img.rgb_data[0..3], &[255, 255, 255]); // 1 = white
        assert_eq!(&img.rgb_data[3..6], &[0, 0, 0]);       // 0 = black
        assert_eq!(&img.rgb_data[6..9], &[255, 255, 255]);
        assert_eq!(&img.rgb_data[9..12], &[0, 0, 0]);
    }

    #[test]
    fn unsupported_bpc() {
        let result = decode_image_xobject(&[], 1, 1, 12, &ColorSpace::DeviceRGB, None);
        assert!(result.is_err());
    }
}
