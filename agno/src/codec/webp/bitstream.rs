use super::bool_enc::BoolEncoder;
use super::quantize::VpxQuantizer;

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

    // Estimate prob_skip_false from skip flag statistics.
    let skip_count = skip_flags.iter().filter(|&&s| s).count();
    let prob_skip_false = if mb_count > 0 {
        let p = ((mb_count - skip_count) * 255 + mb_count / 2) / mb_count;
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
    let token_part = encode_token_partition(y_coeffs, skip_flags);

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
    let key_header = [0x9D, 0x01, 0x2A, w_bytes[0], w_bytes[1], h_bytes[0], h_bytes[1]];

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

    // log2_nbr_of_dct_partitions = 0 (2 bits) → 1 partition
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

    // Coefficient probability update: we use defaults, so no updates needed.
    // The VP8 spec requires writing the coeff prob update flags here.
    // With refresh_entropy_probs=0, the decoder uses default probs and
    // discards any changes after this frame. We write no updates.

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

/// Encode the token partition: quantized DCT/WHT coefficients.
fn encode_token_partition(
    all_coeffs: &[Vec<[i16; 16]>],
    skip_flags: &[bool],
) -> Vec<u8> {
    let mut enc = BoolEncoder::new();

    for (mb_idx, coeffs) in all_coeffs.iter().enumerate() {
        if skip_flags[mb_idx] {
            continue;
        }
        // Block order: Y2 (1), Y (16), U (4), V (4) = 25 blocks
        for block in coeffs {
            encode_coefficient_block(&mut enc, block);
        }
    }

    enc.finish()
}

/// Encode a single 4x4 coefficient block into the token partition.
///
/// Uses a simplified encoding: for each coefficient in zigzag order,
/// encode whether it's the last non-zero (EOB), whether it's zero,
/// or its magnitude and sign. All decisions use prob=128 (equiprobable)
/// for simplicity.
fn encode_coefficient_block(enc: &mut BoolEncoder, coeffs: &[i16; 16]) {
    // VP8 zig-zag scan order
    const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

    // Find last non-zero coefficient position
    let last_nz = ZIGZAG
        .iter()
        .rposition(|&idx| coeffs[idx] != 0)
        .map(|p| p + 1)
        .unwrap_or(0);

    if last_nz == 0 {
        // EOB immediately: encode "no non-zero coefficients"
        enc.put_bit(false, 128); // is_nonzero = false => EOB
        return;
    }

    for (scan_pos, &coeff_idx) in ZIGZAG.iter().enumerate() {
        let val = coeffs[coeff_idx];

        if scan_pos >= last_nz {
            // EOB
            enc.put_bit(false, 128);
            return;
        }

        // Signal: more coefficients follow (or this is non-zero)
        enc.put_bit(true, 128);

        if val == 0 {
            // This coefficient is zero but more follow
            enc.put_bit(false, 128); // is_nonzero = false
        } else {
            // Non-zero coefficient
            enc.put_bit(true, 128); // is_nonzero = true

            let abs_val = val.unsigned_abs() as u32;
            encode_token_value(enc, abs_val);

            // Sign bit
            enc.put_bit(val < 0, 128);
        }
    }
}

/// Encode the magnitude of a non-zero coefficient using VP8-style token categories.
fn encode_token_value(enc: &mut BoolEncoder, abs_val: u32) {
    match abs_val {
        1 => {
            enc.put_bit(false, 128); // category 0: literal 1
        }
        2 => {
            enc.put_bit(true, 128); // not literal 1
            enc.put_bit(false, 128); // category 1: literal 2
        }
        3 => {
            enc.put_bit(true, 128);
            enc.put_bit(true, 128);
            enc.put_bit(false, 128); // literal 3
        }
        4 => {
            enc.put_bit(true, 128);
            enc.put_bit(true, 128);
            enc.put_bit(true, 128);
            enc.put_bit(false, 128); // literal 4
        }
        _ => {
            // For values >= 5, encode using category with extra bits.
            enc.put_bit(true, 128);
            enc.put_bit(true, 128);
            enc.put_bit(true, 128);
            enc.put_bit(true, 128); // signal "large value"

            // Determine number of bits needed and encode magnitude
            let bits_needed = 32 - abs_val.leading_zeros();
            // Encode the bit count (4 bits, max 16)
            enc.put_literal(bits_needed.min(16), 4);
            // Encode the value
            enc.put_literal(abs_val, bits_needed.min(16) as u8);
        }
    }
}

/// Encode luma 16x16 intra mode using the VP8 key-frame mode tree.
///
/// Tree structure:
/// ```text
/// is_not_DC? (prob 145)
///   false -> DC (0)
///   true ->
///     is_not_V? (prob 156)
///       false -> V (1)
///       true ->
///         is_H? (prob 163)
///           false -> TM (3)
///           true -> H (2)
/// ```
fn encode_y_mode(enc: &mut BoolEncoder, mode: u8) {
    match mode {
        0 => {
            // DC
            enc.put_bit(false, 145);
        }
        1 => {
            // V
            enc.put_bit(true, 145);
            enc.put_bit(false, 156);
        }
        2 => {
            // H
            enc.put_bit(true, 145);
            enc.put_bit(true, 156);
            enc.put_bit(true, 163);
        }
        3 => {
            // TM
            enc.put_bit(true, 145);
            enc.put_bit(true, 156);
            enc.put_bit(false, 163);
        }
        _ => {
            // Fallback to DC
            enc.put_bit(false, 145);
        }
    }
}

/// Encode chroma intra mode using the VP8 chroma mode tree.
///
/// Tree structure:
/// ```text
/// is_not_DC? (prob 142)
///   false -> DC (0)
///   true ->
///     is_not_V? (prob 114)
///       false -> V (1)
///       true ->
///         is_H? (prob 183)
///           false -> TM (3)
///           true -> H (2)
/// ```
fn encode_uv_mode(enc: &mut BoolEncoder, mode: u8) {
    match mode {
        0 => {
            enc.put_bit(false, 142);
        }
        1 => {
            enc.put_bit(true, 142);
            enc.put_bit(false, 114);
        }
        2 => {
            enc.put_bit(true, 142);
            enc.put_bit(true, 114);
            enc.put_bit(true, 183);
        }
        3 => {
            enc.put_bit(true, 142);
            enc.put_bit(true, 114);
            enc.put_bit(false, 183);
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
