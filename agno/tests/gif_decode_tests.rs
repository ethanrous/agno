//! Integration tests for GIF decoding via the public agno crate API.

#![cfg(feature = "gif")]

use agno::agno_image::load::load_gif_frame_from_bytes;
use agno::codec::gif::{decode_gif_frame, gif_frame_count};
use agno::exif::ExifContext;

use image::{Frame, RgbaImage, codecs::gif::GifEncoder};
use std::io::Cursor;

/// Encode a sequence of (R,G,B) colored solid frames via the image crate
/// and return the resulting GIF bytes plus the canvas dimensions.
fn make_solid_color_gif(width: u32, height: u32, colors: &[[u8; 3]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut enc = GifEncoder::new(Cursor::new(&mut bytes));
        for &[r, g, b] in colors {
            let mut rgba = RgbaImage::new(width, height);
            for px in rgba.pixels_mut() {
                *px = image::Rgba([r, g, b, 255]);
            }
            enc.encode_frame(Frame::new(rgba)).unwrap();
        }
    }
    bytes
}

#[test]
fn frame_count_matches_input() {
    let bytes = make_solid_color_gif(8, 8, &[[255, 0, 0], [0, 255, 0], [0, 0, 255]]);
    assert_eq!(gif_frame_count(&bytes).unwrap(), 3);
}

#[test]
fn solid_color_decoded_correctly() {
    let bytes = make_solid_color_gif(8, 8, &[[255, 0, 0]]);
    let (rgb, w, h, count) = decode_gif_frame(&bytes, 0).unwrap();
    assert_eq!((w, h, count), (8, 8, 1));
    assert_eq!(rgb.len(), 8 * 8 * 3);
    for px in rgb.chunks_exact(3) {
        // The image crate may quantize, but a single solid color should be exact
        // because the encoder picks that color as a palette entry.
        assert_eq!((px[0], px[1], px[2]), (255, 0, 0));
    }
}

#[test]
fn each_frame_decoded_independently_matches_image_crate() {
    let bytes = make_solid_color_gif(4, 4, &[[200, 100, 50], [10, 20, 30], [60, 60, 60]]);

    let frame_count = gif_frame_count(&bytes).unwrap();
    assert_eq!(frame_count, 3);

    // Decode each frame via the image crate's GIF decoder for comparison.
    let mut reference_frames: Vec<Vec<u8>> = Vec::new();
    {
        use image::AnimationDecoder;
        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(&bytes)).unwrap();
        for frame in decoder.into_frames() {
            let frame = frame.unwrap();
            let buffer = frame.into_buffer();
            // Convert RGBA → RGB
            let rgb: Vec<u8> = buffer
                .pixels()
                .flat_map(|px| [px[0], px[1], px[2]])
                .collect();
            reference_frames.push(rgb);
        }
    }
    assert_eq!(reference_frames.len(), 3);

    for i in 0..3 {
        let (rgb, w, h, _) = decode_gif_frame(&bytes, i).unwrap();
        assert_eq!((w, h), (4, 4));
        // The image crate's animation decoder applies disposal between frames the same way
        // we do (default disposal = 1 keeps the previous frame). For solid-color frames
        // each frame is fully overwritten, so the canvases should match exactly.
        assert_eq!(
            rgb, reference_frames[i],
            "frame {i} differs from image-crate reference"
        );
    }
}

#[test]
fn loader_bridge_sets_page_count() {
    let bytes = make_solid_color_gif(2, 2, &[[1, 2, 3], [4, 5, 6], [7, 8, 9], [10, 11, 12]]);
    let img = load_gif_frame_from_bytes(&bytes, 2, ExifContext::default()).unwrap();
    assert_eq!(img.width, 2);
    assert_eq!(img.height, 2);
    assert_eq!(img.page_count, 4);
    assert_eq!(img.as_slice().len(), 2 * 2 * 3);
}

#[test]
fn larger_canvas_round_trip() {
    // Make a 64x32 single-frame GIF with a horizontal gradient — exercises larger LZW dictionary growth.
    let mut rgba = RgbaImage::new(64, 32);
    for y in 0..32 {
        for x in 0..64 {
            let v = (x * 4) as u8;
            rgba.put_pixel(x, y, image::Rgba([v, 255 - v, 128, 255]));
        }
    }
    let mut bytes = Vec::new();
    {
        let mut enc = GifEncoder::new(Cursor::new(&mut bytes));
        enc.encode_frame(Frame::new(rgba.clone())).unwrap();
    }

    let (rgb, w, h, count) = decode_gif_frame(&bytes, 0).unwrap();
    assert_eq!((w, h, count), (64, 32, 1));
    assert_eq!(rgb.len(), 64 * 32 * 3);

    // Compare against image crate output of the same encoded bytes.
    let reference = image::load_from_memory(&bytes).unwrap().to_rgb8();
    let reference_raw: Vec<u8> = reference.into_raw();
    assert_eq!(rgb, reference_raw);
}
