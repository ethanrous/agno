pub mod dct;
mod decoder_impl;
pub mod encode;
pub mod huffman;
pub mod tables;

pub use encode::encode_jpeg;

use std::error::Error;
use std::io::Cursor;

/// Decode JPEG data to RGB8 (3 bytes/pixel).
/// Returns (rgb_pixels, width, height).
pub fn decode_jpeg(data: &[u8]) -> Result<(Vec<u8>, u32, u32), Box<dyn Error>> {
    let cursor = Cursor::new(data);
    let mut decoder = decoder_impl::Decoder::new(cursor);
    let pixels = decoder.decode()?;
    let info = decoder
        .info()
        .ok_or_else(|| "jpeg decoder returned no image info".to_string())?;

    let width = info.width as u32;
    let height = info.height as u32;

    let rgb = match info.pixel_format {
        decoder_impl::PixelFormat::L8 => {
            // Grayscale → RGB by tripling each byte
            pixels.iter().flat_map(|&y| [y, y, y]).collect()
        }
        decoder_impl::PixelFormat::L16 => {
            // 16-bit grayscale: take high byte of each sample pair → RGB
            pixels
                .chunks_exact(2)
                .flat_map(|pair| {
                    let y = pair[1]; // big-endian: high byte
                    [y, y, y]
                })
                .collect()
        }
        decoder_impl::PixelFormat::RGB24 => pixels,
        decoder_impl::PixelFormat::CMYK32 => {
            // CMYK → RGB: R = C*K/255, G = M*K/255, B = Y*K/255
            pixels
                .chunks_exact(4)
                .flat_map(|cmyk| {
                    let (c, m, y, k) = (
                        cmyk[0] as u32,
                        cmyk[1] as u32,
                        cmyk[2] as u32,
                        cmyk[3] as u32,
                    );
                    [
                        ((255 - c) * k / 255) as u8,
                        ((255 - m) * k / 255) as u8,
                        ((255 - y) * k / 255) as u8,
                    ]
                })
                .collect()
        }
    };

    Ok((rgb, width, height))
}
