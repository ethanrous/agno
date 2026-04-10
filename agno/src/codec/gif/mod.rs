//! Native GIF decoder. Pure Rust, no external C dependencies.
//!
//! Public API mirrors `codec::pdf` because GIFs may have multiple frames:
//! - `decode_gif_frame(data, frame_index)` → `(rgb8, width, height, frame_count)`
//! - `gif_frame_count(data)` → number of frames in the GIF
//!
//! Output is always RGB8 of the logical screen, after compositing all frames
//! up to and including `frame_index` using the standard GIF disposal methods.

mod decode;
mod lzw;

pub use decode::{decode_gif_frame, gif_frame_count};
