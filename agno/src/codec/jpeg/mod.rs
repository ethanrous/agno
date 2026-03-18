pub mod tables;
pub mod dct;
pub mod huffman;
pub mod encode;
pub mod decode;

pub use encode::encode_jpeg;
pub use decode::decode_jpeg;
