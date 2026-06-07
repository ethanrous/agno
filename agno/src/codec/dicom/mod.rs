//! Native DICOM (.dcm) decoder. Pure Rust, no external C dependencies.
//!
//! Supports uncompressed DICOM Part-10 files (Explicit and Implicit VR Little
//! Endian) carrying MONOCHROME1/MONOCHROME2 grayscale (8/16-bit, signed or
//! unsigned) or RGB color images. Output is always RGB8.
//!
//! Pipeline: `parse_dicom` walks the byte stream into a `DicomImage`;
//! `decode_dicom` applies the Modality LUT (rescale), the linear VOI LUT
//! (window/level), and the photometric interpretation to produce RGB8.
//!
//! Public API:
//! - `decode_dicom(data) -> (rgb8, width, height, frame_count)` (frame 0)
//! - `decode_dicom_frame(data, frame_index) -> (rgb8, width, height, frame_count)`
//! - `is_dicom(data) -> bool` (Part-10 magic check)

#[cfg(test)]
pub(crate) mod test_fixtures;

mod decode;
mod parse;
mod voi;

pub use decode::{decode_dicom, decode_dicom_frame};
pub use parse::is_dicom;
