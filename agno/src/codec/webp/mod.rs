pub mod bool_enc;
pub mod bool_dec;
pub mod predict;
pub mod transform;
pub mod quantize;
pub mod bitstream;
pub mod riff;
pub mod encode;
pub mod decode;

pub use encode::encode_webp;
pub use decode::decode_webp;
