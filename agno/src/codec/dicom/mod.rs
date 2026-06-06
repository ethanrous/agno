//! Native DICOM (.dcm) decoder. Pure Rust, no external C dependencies.
//!
//! Supports uncompressed DICOM Part-10 files (Explicit and Implicit VR Little
//! Endian) carrying MONOCHROME1/MONOCHROME2 grayscale (8/16-bit, signed or
//! unsigned) or RGB color images. Output is always RGB8.
//!
//! Pipeline: [`parse::parse_dicom`] walks the byte stream into a [`parse::DicomImage`];
//! [`decode::decode_dicom`] applies the Modality LUT (rescale), the linear VOI LUT
//! (window/level), and the photometric interpretation to produce RGB8.
//!
//! Public API:
//! - `decode_dicom(data) -> (rgb8, width, height, frame_count)`

mod decode;
mod parse;
mod voi;

pub use decode::decode_dicom;
