use super::bool_enc::BoolEncoder;
use super::quantize::VpxQuantizer;

/// VP8 zig-zag scan order for 4x4 blocks.
const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// Map from scan position (0-15) to coefficient band (0-7).
const COEFF_BANDS: [usize; 16] = [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7];

/// Category extra-bit probabilities from RFC 6386.
const CAT1_PROB: [u8; 1] = [159];
const CAT2_PROB: [u8; 2] = [165, 145];
const CAT3_PROB: [u8; 3] = [173, 148, 140];
const CAT4_PROB: [u8; 4] = [176, 155, 140, 135];
const CAT5_PROB: [u8; 5] = [180, 157, 141, 134, 130];
const CAT6_PROB: [u8; 11] = [254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129];

/// Default coefficient probabilities from VP8 spec (libvpx default_coef_probs.h).
/// Indexed as [type][band][ctx][node].
/// - type 0: Y2 (DCT of DC values)
/// - type 1: Y AC (DC is in Y2, starts at scan position 1)
/// - type 2: UV chroma
/// - type 3: Y full (with DC, for B_PRED)
#[rustfmt::skip]
pub const DEFAULT_COEFF_PROBS: [[[[u8; 11]; 3]; 8]; 4] = [
  [ // type 0 (Y2)
    [ // band 0
      [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
      [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
      [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
    ],
    [ // band 1
      [253, 136, 254, 255, 228, 219, 128, 128, 128, 128, 128],
      [189, 129, 242, 255, 227, 213, 255, 219, 128, 128, 128],
      [106, 126, 227, 252, 214, 209, 255, 255, 128, 128, 128],
    ],
    [ // band 2
      [  1,  98, 248, 255, 236, 226, 255, 255, 128, 128, 128],
      [181, 133, 238, 254, 221, 234, 255, 154, 128, 128, 128],
      [ 78, 134, 202, 247, 198, 180, 255, 219, 128, 128, 128],
    ],
    [ // band 3
      [  1, 185, 249, 255, 243, 255, 128, 128, 128, 128, 128],
      [184, 150, 247, 255, 236, 224, 128, 128, 128, 128, 128],
      [ 77, 110, 216, 255, 236, 230, 128, 128, 128, 128, 128],
    ],
    [ // band 4
      [  1, 101, 251, 255, 241, 255, 128, 128, 128, 128, 128],
      [170, 139, 241, 252, 236, 209, 255, 255, 128, 128, 128],
      [ 37, 116, 196, 243, 228, 255, 255, 255, 128, 128, 128],
    ],
    [ // band 5
      [  1, 204, 254, 255, 245, 255, 128, 128, 128, 128, 128],
      [207, 160, 250, 255, 238, 128, 128, 128, 128, 128, 128],
      [102, 103, 231, 255, 211, 171, 128, 128, 128, 128, 128],
    ],
    [ // band 6
      [  1, 152, 252, 255, 240, 255, 128, 128, 128, 128, 128],
      [177, 135, 243, 255, 234, 225, 128, 128, 128, 128, 128],
      [ 80, 129, 211, 255, 194, 224, 128, 128, 128, 128, 128],
    ],
    [ // band 7
      [  1,   1, 255, 128, 128, 128, 128, 128, 128, 128, 128],
      [246,   1, 255, 128, 128, 128, 128, 128, 128, 128, 128],
      [255, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
    ],
  ],
  [ // type 1 (Y AC, DC in Y2)
    [ // band 0
      [198,  35, 237, 223, 193, 187, 162, 160, 145, 155,  62],
      [131,  45, 198, 221, 172, 176, 220, 157, 252, 221,   1],
      [ 68,  47, 146, 208, 149, 167, 221, 162, 255, 223, 128],
    ],
    [ // band 1
      [  1, 149, 241, 255, 221, 224, 255, 255, 128, 128, 128],
      [184, 141, 234, 253, 222, 220, 255, 199, 128, 128, 128],
      [ 81,  99, 181, 242, 176, 190, 249, 202, 255, 255, 128],
    ],
    [ // band 2
      [  1, 129, 232, 253, 214, 197, 242, 196, 255, 255, 128],
      [ 99, 121, 210, 250, 201, 198, 255, 202, 128, 128, 128],
      [ 23,  91, 163, 242, 170, 187, 247, 210, 255, 255, 128],
    ],
    [ // band 3
      [  1, 200, 246, 255, 234, 255, 128, 128, 128, 128, 128],
      [109, 178, 241, 255, 231, 245, 255, 255, 128, 128, 128],
      [ 44, 130, 201, 253, 205, 192, 255, 255, 128, 128, 128],
    ],
    [ // band 4
      [  1, 132, 239, 251, 219, 209, 255, 165, 128, 128, 128],
      [ 94, 136, 225, 251, 218, 190, 255, 255, 128, 128, 128],
      [ 22, 100, 174, 245, 186, 161, 255, 199, 128, 128, 128],
    ],
    [ // band 5
      [  1, 182, 249, 255, 232, 235, 128, 128, 128, 128, 128],
      [124, 143, 241, 255, 227, 234, 128, 128, 128, 128, 128],
      [ 35,  77, 181, 251, 193, 211, 255, 205, 128, 128, 128],
    ],
    [ // band 6
      [  1, 157, 247, 255, 236, 231, 255, 255, 128, 128, 128],
      [121, 141, 235, 255, 225, 227, 255, 255, 128, 128, 128],
      [ 45,  99, 188, 251, 195, 217, 255, 224, 128, 128, 128],
    ],
    [ // band 7
      [  1,   1, 251, 255, 213, 255, 128, 128, 128, 128, 128],
      [203,   1, 248, 255, 255, 128, 128, 128, 128, 128, 128],
      [137,   1, 177, 255, 224, 255, 128, 128, 128, 128, 128],
    ],
  ],
  [ // type 2 (UV chroma)
    [ // band 0
      [253,   9, 248, 251, 207, 208, 255, 192, 128, 128, 128],
      [175,  13, 224, 243, 193, 185, 249, 198, 255, 255, 128],
      [ 73,  17, 171, 221, 161, 179, 236, 167, 255, 234, 128],
    ],
    [ // band 1
      [  1,  95, 247, 253, 212, 183, 255, 255, 128, 128, 128],
      [239,  90, 244, 250, 211, 209, 255, 255, 128, 128, 128],
      [155,  77, 195, 248, 188, 195, 255, 255, 128, 128, 128],
    ],
    [ // band 2
      [  1,  24, 239, 251, 218, 219, 255, 205, 128, 128, 128],
      [201,  51, 219, 255, 196, 186, 128, 128, 128, 128, 128],
      [ 69,  46, 190, 239, 201, 218, 255, 228, 128, 128, 128],
    ],
    [ // band 3
      [  1, 191, 251, 255, 255, 128, 128, 128, 128, 128, 128],
      [223, 165, 249, 255, 213, 255, 128, 128, 128, 128, 128],
      [141, 124, 248, 255, 255, 128, 128, 128, 128, 128, 128],
    ],
    [ // band 4
      [  1,  16, 248, 255, 255, 128, 128, 128, 128, 128, 128],
      [190,  36, 230, 255, 236, 255, 128, 128, 128, 128, 128],
      [149,   1, 255, 128, 128, 128, 128, 128, 128, 128, 128],
    ],
    [ // band 5
      [  1, 226, 255, 128, 128, 128, 128, 128, 128, 128, 128],
      [247, 192, 255, 128, 128, 128, 128, 128, 128, 128, 128],
      [240, 128, 255, 128, 128, 128, 128, 128, 128, 128, 128],
    ],
    [ // band 6
      [  1, 134, 252, 255, 255, 128, 128, 128, 128, 128, 128],
      [213,  62, 250, 255, 255, 128, 128, 128, 128, 128, 128],
      [ 55,  93, 255, 128, 128, 128, 128, 128, 128, 128, 128],
    ],
    [ // band 7
      [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
      [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
      [128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128],
    ],
  ],
  [ // type 3 (Y full with DC, for B_PRED)
    [ // band 0
      [202,  24, 213, 235, 186, 191, 220, 160, 240, 175, 255],
      [126,  38, 182, 232, 169, 184, 228, 174, 255, 187, 128],
      [ 61,  46, 138, 219, 151, 178, 240, 170, 255, 216, 128],
    ],
    [ // band 1
      [  1, 112, 230, 250, 199, 191, 247, 159, 255, 255, 128],
      [166, 109, 228, 252, 211, 215, 255, 174, 128, 128, 128],
      [ 39,  77, 162, 232, 172, 180, 245, 178, 255, 255, 128],
    ],
    [ // band 2
      [  1,  52, 220, 246, 198, 199, 249, 220, 255, 255, 128],
      [124,  74, 191, 243, 183, 193, 250, 221, 255, 255, 128],
      [ 24,  71, 130, 219, 154, 170, 243, 182, 255, 255, 128],
    ],
    [ // band 3
      [  1, 182, 225, 249, 219, 240, 255, 224, 128, 128, 128],
      [149, 150, 226, 252, 216, 205, 255, 171, 128, 128, 128],
      [ 28, 108, 170, 242, 183, 194, 254, 223, 255, 255, 128],
    ],
    [ // band 4
      [  1,  81, 230, 252, 204, 203, 255, 192, 128, 128, 128],
      [123, 102, 209, 247, 188, 196, 255, 233, 128, 128, 128],
      [ 20,  95, 153, 243, 164, 173, 255, 203, 128, 128, 128],
    ],
    [ // band 5
      [  1, 222, 248, 255, 216, 213, 128, 128, 128, 128, 128],
      [168, 175, 246, 252, 235, 205, 255, 255, 128, 128, 128],
      [ 47, 116, 215, 255, 211, 212, 255, 255, 128, 128, 128],
    ],
    [ // band 6
      [  1, 121, 236, 253, 212, 214, 255, 255, 128, 128, 128],
      [141,  84, 213, 252, 201, 202, 255, 219, 128, 128, 128],
      [ 42,  80, 160, 240, 162, 185, 255, 205, 128, 128, 128],
    ],
    [ // band 7
      [  1,   1, 255, 128, 128, 128, 128, 128, 128, 128, 128],
      [244,   1, 255, 128, 128, 128, 128, 128, 128, 128, 128],
      [238,   1, 255, 128, 128, 128, 128, 128, 128, 128, 128],
    ],
  ],
];

/// Coefficient probability update probabilities from VP8 spec (libvpx coefupdateprobs.h).
/// Used when writing the coefficient probability update section of the first partition.
#[rustfmt::skip]
pub const COEFF_UPDATE_PROBS: [[[[u8; 11]; 3]; 8]; 4] = [
  [
    [
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [176, 246, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [223, 241, 252, 255, 255, 255, 255, 255, 255, 255, 255],
      [249, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 244, 252, 255, 255, 255, 255, 255, 255, 255, 255],
      [234, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 246, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [239, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [254, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [251, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [251, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [254, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 254, 253, 255, 254, 255, 255, 255, 255, 255, 255],
      [250, 255, 254, 255, 254, 255, 255, 255, 255, 255, 255],
      [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
  ],
  [
    [
      [217, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [225, 252, 241, 253, 255, 255, 254, 255, 255, 255, 255],
      [234, 250, 241, 250, 253, 255, 253, 254, 255, 255, 255],
    ],
    [
      [255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [223, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [238, 253, 254, 254, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 248, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [249, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 253, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [247, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [252, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [253, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 254, 253, 255, 255, 255, 255, 255, 255, 255, 255],
      [250, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
  ],
  [
    [
      [186, 251, 250, 255, 255, 255, 255, 255, 255, 255, 255],
      [234, 251, 244, 254, 255, 255, 255, 255, 255, 255, 255],
      [251, 251, 243, 253, 254, 255, 254, 255, 255, 255, 255],
    ],
    [
      [255, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [236, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [251, 253, 253, 254, 254, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [254, 254, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [254, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
  ],
  [
    [
      [248, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [250, 254, 252, 254, 255, 255, 255, 255, 255, 255, 255],
      [248, 254, 249, 253, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255],
      [246, 253, 253, 255, 255, 255, 255, 255, 255, 255, 255],
      [252, 254, 251, 254, 254, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 254, 252, 255, 255, 255, 255, 255, 255, 255, 255],
      [248, 254, 253, 255, 255, 255, 255, 255, 255, 255, 255],
      [253, 255, 254, 254, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 251, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [245, 251, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [253, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 251, 253, 255, 255, 255, 255, 255, 255, 255, 255],
      [252, 253, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 254, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 252, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [249, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 254, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 255, 253, 255, 255, 255, 255, 255, 255, 255, 255],
      [250, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
    [
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [254, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
      [255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    ],
  ],
];

/// Encode a complete VP8 key-frame bitstream.
///
/// Returns the raw VP8 frame data (to be wrapped in RIFF by `riff::wrap_riff_webp`).
///
/// # Arguments
/// * `width`, `height` - Image dimensions in pixels.
/// * `quantizer` - Quantization parameters.
/// * `mb_modes_y` - 16x16 luma intra mode per macroblock (row-major).
/// * `mb_modes_uv` - Chroma intra mode per macroblock.
/// * `y_coeffs` - Per-MB coefficient blocks: `[Y2(1), Y(16), U(4), V(4)]` = 25 blocks.
/// * `skip_flags` - Per-MB: true if all quantized coefficients are zero.
pub fn encode_vp8_frame(
    width: u32,
    height: u32,
    quantizer: &VpxQuantizer,
    mb_modes_y: &[u8],
    mb_modes_uv: &[u8],
    y_coeffs: &[Vec<[i16; 16]>],
    skip_flags: &[bool],
) -> Vec<u8> {
    let mb_count = mb_modes_y.len();
    let q_index = find_qindex(quantizer);
    let mb_cols = width.div_ceil(16) as usize;

    // Estimate prob_skip_false from skip flag statistics.
    let skip_count = skip_flags.iter().filter(|&&s| s).count();
    let prob_skip_false = if mb_count > 0 {
        let p = ((mb_count - skip_count) * 255 + mb_count / 2)
            .checked_div(mb_count)
            .unwrap_or(0);
        p.clamp(1, 255) as u8
    } else {
        255
    };

    // --- First partition: frame header + macroblock modes ---
    let first_part = encode_first_partition(
        width,
        height,
        q_index,
        prob_skip_false,
        mb_modes_y,
        mb_modes_uv,
        skip_flags,
    );

    // --- Token partition: coefficient data ---
    let token_part = encode_token_partition(y_coeffs, skip_flags, mb_cols);

    // --- Assemble the frame ---
    let first_part_size = first_part.len() as u32;

    // Frame tag (3 bytes):
    //   bit 0        = 0 (key frame)
    //   bits 1-3     = version (0)
    //   bit 4        = 1 (show_frame)
    //   bits 5-23    = first_part_size
    let tag0 = (first_part_size << 5) | 0x10; // show_frame=1, version=0, key=0
    let tag_bytes = [
        (tag0 & 0xFF) as u8,
        ((tag0 >> 8) & 0xFF) as u8,
        ((tag0 >> 16) & 0xFF) as u8,
    ];

    // Key frame header (7 bytes):
    //   3 bytes start code: 0x9D 0x01 0x2A
    //   2 bytes width LE (bits 0-13 = width, bits 14-15 = hscale=0)
    //   2 bytes height LE (bits 0-13 = height, bits 14-15 = vscale=0)
    let w_bytes = (width & 0x3FFF).to_le_bytes();
    let h_bytes = (height & 0x3FFF).to_le_bytes();
    let key_header = [
        0x9D, 0x01, 0x2A, w_bytes[0], w_bytes[1], h_bytes[0], h_bytes[1],
    ];

    let total = 3 + 7 + first_part.len() + token_part.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&tag_bytes);
    out.extend_from_slice(&key_header);
    out.extend_from_slice(&first_part);
    out.extend_from_slice(&token_part);
    out
}

/// Encode the first partition: frame header parameters + macroblock prediction modes.
fn encode_first_partition(
    _width: u32,
    _height: u32,
    q_index: u8,
    prob_skip_false: u8,
    mb_modes_y: &[u8],
    mb_modes_uv: &[u8],
    skip_flags: &[bool],
) -> Vec<u8> {
    let mut enc = BoolEncoder::new();

    // color_space = 0 (YCbCr)
    enc.put_bit(false, 128);
    // clamping_type = 0
    enc.put_bit(false, 128);

    // segmentation_enabled = 0
    enc.put_bit(false, 128);

    // filter_type = 0 (normal)
    enc.put_bit(false, 128);
    // filter_level = 0 (6 bits)
    enc.put_literal(0, 6);
    // sharpness = 0 (3 bits)
    enc.put_literal(0, 3);

    // lf_delta_enabled = 0
    enc.put_bit(false, 128);

    // log2_nbr_of_dct_partitions = 0 (2 bits) -> 1 partition
    enc.put_literal(0, 2);

    // Quantization parameters:
    // y_ac_qi (7 bits)
    enc.put_literal(q_index as u32, 7);
    // y_dc_delta_present = 0
    enc.put_bit(false, 128);
    // y2_dc_delta_present = 0
    enc.put_bit(false, 128);
    // y2_ac_delta_present = 0
    enc.put_bit(false, 128);
    // uv_dc_delta_present = 0
    enc.put_bit(false, 128);
    // uv_ac_delta_present = 0
    enc.put_bit(false, 128);

    // refresh_entropy_probs = 0
    enc.put_bit(false, 128);

    // Coefficient probability update section (RFC 6386 Section 13.4):
    // 4 types * 8 bands * 3 contexts * 11 nodes = 1056 flags.
    // Each flag is written with the corresponding update probability.
    // We send all false (no updates = use defaults).
    for probs_t in &COEFF_UPDATE_PROBS {
        for probs_b in probs_t {
            for probs_c in probs_b {
                for &prob_n in probs_c {
                    enc.put_bit(false, prob_n);
                }
            }
        }
    }

    // mb_no_skip_coeff = 1 (we use skip flags)
    enc.put_bit(true, 128);
    // prob_skip_false
    enc.put_literal(prob_skip_false as u32, 8);

    // Macroblock modes
    for i in 0..mb_modes_y.len() {
        // Encode skip flag first
        enc.put_bit(skip_flags[i], prob_skip_false);

        // Encode luma 16x16 mode via key-frame intra mode tree
        encode_y_mode(&mut enc, mb_modes_y[i]);

        // Encode chroma mode
        encode_uv_mode(&mut enc, mb_modes_uv[i]);
    }

    enc.finish()
}

/// Encode the token partition using VP8-compliant coefficient token encoding.
///
/// Tracks inter-block context (complexity) to match what the standard decoder expects.
/// The complexity for each block's initial context is derived from neighboring blocks:
/// `complexity = top_complexity + left_complexity` (each 0 or 1), clamped to 0..2.
fn encode_token_partition(
    all_coeffs: &[Vec<[i16; 16]>],
    skip_flags: &[bool],
    mb_cols: usize,
) -> Vec<u8> {
    let mut enc = BoolEncoder::new();
    let mb_count = all_coeffs.len();

    // Track top-row complexities per macroblock column.
    // Layout per column: [0]=Y2, [1..5]=Y columns, [5..7]=U, [7..9]=V
    let mut top = vec![[0u8; 9]; mb_cols];

    // Track left complexities within the current row.
    // Same layout: [0]=Y2, [1..5]=Y rows (used as left for Y), [5..7]=U rows, [7..9]=V rows
    let mut left = [0u8; 9];

    for mb_idx in 0..mb_count {
        let mb_col = mb_idx % mb_cols;

        // Reset left complexities at each row start
        if mb_col == 0 {
            left = [0; 9];
        }

        if skip_flags[mb_idx] {
            // Skipped MBs: all complexities become 0
            top[mb_col] = [0; 9];
            left = [0; 9];
            continue;
        }

        let coeffs = &all_coeffs[mb_idx];

        // Y2 block (type=1, starts at scan position 0)
        let y2_init = (top[mb_col][0] + left[0]).min(2) as usize;
        let y2_has = encode_coefficient_block_vp8(&mut enc, &coeffs[0], 1, 0, y2_init);
        let y2_c = u8::from(y2_has);
        left[0] = y2_c;
        top[mb_col][0] = y2_c;

        // Y sub-blocks: 4 rows of 4 (type=0, starts at scan position 1)
        for y in 0..4usize {
            let mut row_left = left[y + 1];
            for x in 0..4usize {
                let blk_idx = y * 4 + x;
                let init = (top[mb_col][x + 1] + row_left).min(2) as usize;
                let has = encode_coefficient_block_vp8(&mut enc, &coeffs[1 + blk_idx], 0, 1, init);
                let c = u8::from(has);
                row_left = c;
                top[mb_col][x + 1] = c;
            }
            left[y + 1] = row_left;
        }

        // U sub-blocks: 2 rows of 2 (type=2, starts at scan position 0)
        for y in 0..2usize {
            let mut row_left = left[y + 5];
            for x in 0..2usize {
                let blk_idx = y * 2 + x;
                let init = (top[mb_col][x + 5] + row_left).min(2) as usize;
                let has = encode_coefficient_block_vp8(&mut enc, &coeffs[17 + blk_idx], 2, 0, init);
                let c = u8::from(has);
                row_left = c;
                top[mb_col][x + 5] = c;
            }
            left[y + 5] = row_left;
        }

        // V sub-blocks: 2 rows of 2 (type=2, starts at scan position 0)
        for y in 0..2usize {
            let mut row_left = left[y + 7];
            for x in 0..2usize {
                let blk_idx = y * 2 + x;
                let init = (top[mb_col][x + 7] + row_left).min(2) as usize;
                let has = encode_coefficient_block_vp8(&mut enc, &coeffs[21 + blk_idx], 2, 0, init);
                let c = u8::from(has);
                row_left = c;
                top[mb_col][x + 7] = c;
            }
            left[y + 7] = row_left;
        }
    }

    enc.finish()
}

/// Encode a single 4x4 coefficient block using the VP8 token tree.
///
/// `coeff_type`: 0=Y(AC), 1=Y2, 2=UV, 3=Y(full/B_PRED)
/// `first_coeff`: index of the first coefficient in zigzag order (0 for most, 1 for Y AC)
/// `init_ctx`: initial context derived from neighbor block complexity (0, 1, or 2)
///
/// Returns `true` if the block had any non-zero coefficients (used for context tracking).
///
/// The VP8 token tree and context tracking follow RFC 6386 Section 13:
/// - After a ZERO token, the next coefficient skips the EOB node (node 0)
///   and starts directly at node 1 (the zero/nonzero decision).
/// - Context (for prob table lookup): 0 after zero, 1 after abs(val)==1, 2 after abs(val)>1.
fn encode_coefficient_block_vp8(
    enc: &mut BoolEncoder,
    coeffs: &[i16; 16],
    coeff_type: usize,
    first_coeff: usize,
    init_ctx: usize,
) -> bool {
    let has_coefficients = ZIGZAG[first_coeff..].iter().any(|&idx| coeffs[idx] != 0);

    // Find last non-zero coefficient position in scan order
    let last_nz = ZIGZAG[first_coeff..]
        .iter()
        .rposition(|&idx| coeffs[idx] != 0)
        .map(|p| first_coeff + p + 1)
        .unwrap_or(first_coeff);

    // Initial context from inter-block complexity
    let mut ctx: usize = init_ctx;
    // After a ZERO token, skip the EOB node for the next coefficient
    let mut skip_eob = false;

    for scan_pos in first_coeff..16 {
        let coeff_idx = ZIGZAG[scan_pos];
        let val = coeffs[coeff_idx];
        let band = COEFF_BANDS[scan_pos];
        let probs = &DEFAULT_COEFF_PROBS[coeff_type][band][ctx];

        if scan_pos >= last_nz {
            if !skip_eob {
                // EOB: node 0 = false
                enc.put_bit(false, probs[0]);
            }
            return has_coefficients;
        }

        if !skip_eob {
            // Not EOB: node 0 = true
            enc.put_bit(true, probs[0]);
        }

        if val == 0 {
            // ZERO token: node 1 = false
            enc.put_bit(false, probs[1]);
            ctx = 0;
            skip_eob = true;
        } else {
            // Non-zero: node 1 = true
            enc.put_bit(true, probs[1]);

            let abs_val = val.unsigned_abs() as u32;
            encode_token_tree(enc, probs, abs_val);

            // Sign bit (always at prob=128)
            enc.put_bit(val < 0, 128);

            ctx = if abs_val == 1 { 1 } else { 2 };
            skip_eob = false;
        }
    }
    // All 16 coefficients written, no EOB needed (implied at end of block)
    has_coefficients
}

/// Encode the magnitude of a non-zero coefficient using the VP8 token tree.
///
/// VP8 DCT token tree (RFC 6386 Section 13.2), indexed by probability node:
///   Node 2 (prob[2]): false=ONE(1), true -> Node 3
///   Node 3 (prob[3]): false -> Node 4 (TWO-FOUR), true -> Node 6 (CAT1-6)
///   Node 4 (prob[4]): false=TWO(2), true -> Node 5
///   Node 5 (prob[5]): false=THREE(3), true=FOUR(4)
///   Node 6 (prob[6]): false -> Node 7 (CAT1-2), true -> Node 8 (CAT3-6)
///   Node 7 (prob[7]): false=CAT1(5+extra), true=CAT2(7+extra)
///   Node 8 (prob[8]): false -> Node 9 (CAT3-4), true -> Node 10 (CAT5-6)
///   Node 9 (prob[9]): false=CAT3(11+extra), true=CAT4(19+extra)
///   Node 10 (prob[10]): false=CAT5(35+extra), true=CAT6(67+extra)
fn encode_token_tree(enc: &mut BoolEncoder, probs: &[u8; 11], abs_val: u32) {
    match abs_val {
        1 => {
            // ONE: node2=false
            enc.put_bit(false, probs[2]);
        }
        2 => {
            // TWO: node2=true, node3=false, node4=false
            enc.put_bit(true, probs[2]);
            enc.put_bit(false, probs[3]);
            enc.put_bit(false, probs[4]);
        }
        3 => {
            // THREE: node2=true, node3=false, node4=true, node5=false
            enc.put_bit(true, probs[2]);
            enc.put_bit(false, probs[3]);
            enc.put_bit(true, probs[4]);
            enc.put_bit(false, probs[5]);
        }
        4 => {
            // FOUR: node2=true, node3=false, node4=true, node5=true
            enc.put_bit(true, probs[2]);
            enc.put_bit(false, probs[3]);
            enc.put_bit(true, probs[4]);
            enc.put_bit(true, probs[5]);
        }
        5..=6 => {
            // CAT1: base=5, 1 extra bit
            // node2=true, node3=true, node6=false, node7=false
            enc.put_bit(true, probs[2]);
            enc.put_bit(true, probs[3]);
            enc.put_bit(false, probs[6]);
            enc.put_bit(false, probs[7]);
            let extra = abs_val - 5;
            enc.put_bit(extra & 1 != 0, CAT1_PROB[0]);
        }
        7..=10 => {
            // CAT2: base=7, 2 extra bits
            // node2=true, node3=true, node6=false, node7=true
            enc.put_bit(true, probs[2]);
            enc.put_bit(true, probs[3]);
            enc.put_bit(false, probs[6]);
            enc.put_bit(true, probs[7]);
            let extra = abs_val - 7;
            enc.put_bit(extra >> 1 & 1 != 0, CAT2_PROB[0]);
            enc.put_bit(extra & 1 != 0, CAT2_PROB[1]);
        }
        11..=18 => {
            // CAT3: base=11, 3 extra bits
            // node2=true, node3=true, node6=true, node8=false, node9=false
            enc.put_bit(true, probs[2]);
            enc.put_bit(true, probs[3]);
            enc.put_bit(true, probs[6]);
            enc.put_bit(false, probs[8]);
            enc.put_bit(false, probs[9]);
            let extra = abs_val - 11;
            enc.put_bit(extra >> 2 & 1 != 0, CAT3_PROB[0]);
            enc.put_bit(extra >> 1 & 1 != 0, CAT3_PROB[1]);
            enc.put_bit(extra & 1 != 0, CAT3_PROB[2]);
        }
        19..=34 => {
            // CAT4: base=19, 4 extra bits
            // node2=true, node3=true, node6=true, node8=false, node9=true
            enc.put_bit(true, probs[2]);
            enc.put_bit(true, probs[3]);
            enc.put_bit(true, probs[6]);
            enc.put_bit(false, probs[8]);
            enc.put_bit(true, probs[9]);
            let extra = abs_val - 19;
            enc.put_bit(extra >> 3 & 1 != 0, CAT4_PROB[0]);
            enc.put_bit(extra >> 2 & 1 != 0, CAT4_PROB[1]);
            enc.put_bit(extra >> 1 & 1 != 0, CAT4_PROB[2]);
            enc.put_bit(extra & 1 != 0, CAT4_PROB[3]);
        }
        35..=66 => {
            // CAT5: base=35, 5 extra bits
            // node2=true, node3=true, node6=true, node8=true, node10=false
            enc.put_bit(true, probs[2]);
            enc.put_bit(true, probs[3]);
            enc.put_bit(true, probs[6]);
            enc.put_bit(true, probs[8]);
            enc.put_bit(false, probs[10]);
            let extra = abs_val - 35;
            enc.put_bit(extra >> 4 & 1 != 0, CAT5_PROB[0]);
            enc.put_bit(extra >> 3 & 1 != 0, CAT5_PROB[1]);
            enc.put_bit(extra >> 2 & 1 != 0, CAT5_PROB[2]);
            enc.put_bit(extra >> 1 & 1 != 0, CAT5_PROB[3]);
            enc.put_bit(extra & 1 != 0, CAT5_PROB[4]);
        }
        _ => {
            // CAT6: base=67, 11 extra bits
            // node2=true, node3=true, node6=true, node8=true, node10=true
            enc.put_bit(true, probs[2]);
            enc.put_bit(true, probs[3]);
            enc.put_bit(true, probs[6]);
            enc.put_bit(true, probs[8]);
            enc.put_bit(true, probs[10]);
            let extra = abs_val - 67;
            for (i, &prob) in CAT6_PROB.iter().enumerate() {
                enc.put_bit(extra >> (10 - i) & 1 != 0, prob);
            }
        }
    }
}

/// Encode luma 16x16 intra mode using the VP8 key-frame mode tree.
///
/// KEYFRAME_YMODE_TREE = [-B_PRED, 2, 4, 6, -DC, -V, -H, -TM]
/// Probs = [145, 156, 163, 128]
///
/// Node 0 (prob 145): false=B_PRED(4), true -> Node 1
/// Node 1 (prob 156): false -> Node 2, true -> Node 3
/// Node 2 (prob 163): false=DC(0), true=V(1)
/// Node 3 (prob 128): false=H(2), true=TM(3)
///
/// We never encode B_PRED (mode 4) since our encoder only uses 16x16 modes.
fn encode_y_mode(enc: &mut BoolEncoder, mode: u8) {
    match mode {
        0 => {
            // DC: true@145, false@156, false@163
            enc.put_bit(true, 145);
            enc.put_bit(false, 156);
            enc.put_bit(false, 163);
        }
        1 => {
            // V: true@145, false@156, true@163
            enc.put_bit(true, 145);
            enc.put_bit(false, 156);
            enc.put_bit(true, 163);
        }
        2 => {
            // H: true@145, true@156, false@128
            enc.put_bit(true, 145);
            enc.put_bit(true, 156);
            enc.put_bit(false, 128);
        }
        3 => {
            // TM: true@145, true@156, true@128
            enc.put_bit(true, 145);
            enc.put_bit(true, 156);
            enc.put_bit(true, 128);
        }
        _ => {
            // Fallback to DC
            enc.put_bit(true, 145);
            enc.put_bit(false, 156);
            enc.put_bit(false, 163);
        }
    }
}

/// Encode chroma intra mode using the VP8 key-frame UV mode tree.
///
/// KEYFRAME_UV_MODE_TREE = [-DC, 2, -V, 4, -H, -TM]
/// Probs = [142, 114, 183]
///
/// Node 0 (prob 142): false=DC(0), true -> Node 1
/// Node 1 (prob 114): false=V(1), true -> Node 2
/// Node 2 (prob 183): false=H(2), true=TM(3)
fn encode_uv_mode(enc: &mut BoolEncoder, mode: u8) {
    match mode {
        0 => {
            // DC: false@142
            enc.put_bit(false, 142);
        }
        1 => {
            // V: true@142, false@114
            enc.put_bit(true, 142);
            enc.put_bit(false, 114);
        }
        2 => {
            // H: true@142, true@114, false@183
            enc.put_bit(true, 142);
            enc.put_bit(true, 114);
            enc.put_bit(false, 183);
        }
        3 => {
            // TM: true@142, true@114, true@183
            enc.put_bit(true, 142);
            enc.put_bit(true, 114);
            enc.put_bit(true, 183);
        }
        _ => {
            enc.put_bit(false, 142);
        }
    }
}

/// Reverse-lookup the q_index from quantizer tables.
/// Returns the closest q_index whose DC_QUANT matches y1_dc.
fn find_qindex(quantizer: &VpxQuantizer) -> u8 {
    let target = quantizer.y1_dc;
    super::quantize::DC_QUANT
        .iter()
        .enumerate()
        .min_by_key(|&(_, &v)| (v - target).unsigned_abs())
        .map(|(i, _)| i as u8)
        .unwrap_or(0)
}
