//! Embedded jpeg-decoder implementation.
//! Source: https://github.com/image-rs/jpeg-decoder (MIT/Apache-2.0)
//! Vendored to avoid an external crate dependency while gaining progressive/lossless JPEG support.

#![allow(dead_code, unused_imports, clippy::all, warnings)]

mod arch;
mod decoder;
mod error;
mod huffman;
mod idct;
mod marker;
mod parser;
mod upsampler;
mod worker;

pub(super) use decoder::{ColorTransform, Decoder, ImageInfo, PixelFormat};
pub(super) use error::{Error, UnsupportedFeature};
pub(super) use parser::CodingProcess;

use std::io;

fn read_u8<R: io::Read>(reader: &mut R) -> io::Result<u8> {
    let mut buf = [0];
    reader.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_u16_from_be<R: io::Read>(reader: &mut R) -> io::Result<u16> {
    let mut buf = [0, 0];
    reader.read_exact(&mut buf)?;
    Ok(u16::from_be_bytes(buf))
}
