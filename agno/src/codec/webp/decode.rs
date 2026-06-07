use std::error::Error;

use super::bitstream::{COEFF_UPDATE_PROBS, DEFAULT_COEFF_PROBS};
use super::bool_dec::BoolDecoder;
use super::predict::{predict_16x16, predict_chroma_8x8};
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

/// Decode a lossy VP8 WebP image from raw file bytes.
///
/// Returns `(rgb, width, height)` where `rgb` is interleaved R,G,B with 3
/// bytes per pixel in row-major order.
pub fn decode_webp(data: &[u8]) -> Result<(Vec<u8>, u32, u32), Box<dyn Error>> {
    let vp8_data = parse_riff_webp(data)?;
    let (header, first_part_data, token_part_data, coeff_probs) = parse_frame_header(vp8_data)?;

    let mb_width = (header.width as usize).div_ceil(16);
    let mb_height = (header.height as usize).div_ceil(16);
    let mb_count = mb_width * mb_height;

    let quantizer = VpxQuantizer::new(header.q_index);

    // Decode first partition: skip flags, Y modes, UV modes.
    let (skip_flags, mb_y_modes, mb_uv_modes) =
        decode_first_partition(first_part_data, mb_count, header.prob_skip_false)?;

    // Allocate YUV planes (macroblock-aligned dimensions).
    let y_stride = mb_width * 16;
    let uv_stride = mb_width * 8;
    let y_h = mb_height * 16;
    let uv_h = mb_height * 8;

    let mut y_plane = vec![0u8; y_stride * y_h];
    let mut u_plane = vec![0u8; uv_stride * uv_h];
    let mut v_plane = vec![0u8; uv_stride * uv_h];

    // Decode token partition and reconstruct macroblocks.
    let mut token_dec = BoolDecoder::new(token_part_data);

    // Inter-block complexity tracking (matches encoder and standard decoder)
    let mut top_complexity = vec![[0u8; 9]; mb_width];
    let mut left_complexity = [0u8; 9];

    for mb_idx in 0..mb_count {
        let mb_row = mb_idx / mb_width;
        let mb_col = mb_idx % mb_width;
        let has_above = mb_row > 0;
        let has_left = mb_col > 0;

        // Gather Y prediction context.
        let (y_above, y_left, y_tl) = gather_borders_16(&y_plane, y_stride, mb_row, mb_col);
        let y_pred = predict_16x16(
            &y_above,
            &y_left,
            y_tl,
            mb_y_modes[mb_idx],
            has_above,
            has_left,
        );

        // Gather UV prediction context.
        let (u_above, u_left, u_tl) = gather_borders_8(&u_plane, uv_stride, mb_row, mb_col);
        let (v_above, v_left, v_tl) = gather_borders_8(&v_plane, uv_stride, mb_row, mb_col);
        let u_pred = predict_chroma_8x8(
            &u_above,
            &u_left,
            u_tl,
            mb_uv_modes[mb_idx],
            has_above,
            has_left,
        );
        let v_pred = predict_chroma_8x8(
            &v_above,
            &v_left,
            v_tl,
            mb_uv_modes[mb_idx],
            has_above,
            has_left,
        );

        // Reset left complexity at row start
        if mb_col == 0 {
            left_complexity = [0; 9];
        }

        if skip_flags[mb_idx] {
            write_pred_y(&mut y_plane, y_stride, mb_row, mb_col, &y_pred);
            write_pred_uv(&mut u_plane, uv_stride, mb_row, mb_col, &u_pred);
            write_pred_uv(&mut v_plane, uv_stride, mb_row, mb_col, &v_pred);
            top_complexity[mb_col] = [0; 9];
            left_complexity = [0; 9];
            continue;
        }

        // Decode 25 coefficient blocks using VP8 token tree with context tracking.
        // Y2 block (type=1, starts at scan position 0)
        let y2_init = (top_complexity[mb_col][0] + left_complexity[0]).min(2) as usize;
        let (y2_block, y2_has) =
            decode_coefficient_block_vp8(&mut token_dec, &coeff_probs, 1, 0, y2_init);
        let y2_c = u8::from(y2_has);
        left_complexity[0] = y2_c;
        top_complexity[mb_col][0] = y2_c;

        // Y sub-blocks (type=0, starts at scan position 1)
        let mut y_blocks = [[0i16; 16]; 16];
        for y in 0..4usize {
            let mut row_left = left_complexity[y + 1];
            for x in 0..4usize {
                let blk_idx = y * 4 + x;
                let init = (top_complexity[mb_col][x + 1] + row_left).min(2) as usize;
                let (blk, has) =
                    decode_coefficient_block_vp8(&mut token_dec, &coeff_probs, 0, 1, init);
                y_blocks[blk_idx] = blk;
                let c = u8::from(has);
                row_left = c;
                top_complexity[mb_col][x + 1] = c;
            }
            left_complexity[y + 1] = row_left;
        }

        // U sub-blocks (type=2, starts at scan position 0)
        let mut u_blocks = [[0i16; 16]; 4];
        for y in 0..2usize {
            let mut row_left = left_complexity[y + 5];
            for x in 0..2usize {
                let blk_idx = y * 2 + x;
                let init = (top_complexity[mb_col][x + 5] + row_left).min(2) as usize;
                let (blk, has) =
                    decode_coefficient_block_vp8(&mut token_dec, &coeff_probs, 2, 0, init);
                u_blocks[blk_idx] = blk;
                let c = u8::from(has);
                row_left = c;
                top_complexity[mb_col][x + 5] = c;
            }
            left_complexity[y + 5] = row_left;
        }

        // V sub-blocks (type=2, starts at scan position 0)
        let mut v_blocks = [[0i16; 16]; 4];
        for y in 0..2usize {
            let mut row_left = left_complexity[y + 7];
            for x in 0..2usize {
                let blk_idx = y * 2 + x;
                let init = (top_complexity[mb_col][x + 7] + row_left).min(2) as usize;
                let (blk, has) =
                    decode_coefficient_block_vp8(&mut token_dec, &coeff_probs, 2, 0, init);
                v_blocks[blk_idx] = blk;
                let c = u8::from(has);
                row_left = c;
                top_complexity[mb_col][x + 7] = c;
            }
            left_complexity[y + 7] = row_left;
        }

        // Y2: dequantize with y2_dc/y2_ac, then inverse WHT
        // to recover the 16 DC values for the Y sub-blocks.
        let y2_dc_values = iwht4x4_dequant_dc_ac(&y2_block, quantizer.y2_dc, quantizer.y2_ac);

        // Reconstruct 16 Y sub-blocks.
        for by in 0..4 {
            for bx in 0..4 {
                let blk_idx = by * 4 + bx;
                // DC comes from Y2 (already dequantized through WHT).
                // AC coefficients need per-coefficient dequantization.
                let mut dequant = [0i32; 16];
                dequant[0] = y2_dc_values[blk_idx] as i32;
                for i in 1..16 {
                    dequant[i] = y_blocks[blk_idx][i] as i32 * quantizer.y1_ac as i32;
                }
                let pixels = idct4x4(&dequant);

                let py = mb_row * 16 + by * 4;
                let px = mb_col * 16 + bx * 4;
                for r in 0..4 {
                    for c in 0..4 {
                        let pred = y_pred[(by * 4 + r) * 16 + bx * 4 + c] as i32;
                        let val = (pred + pixels[r * 4 + c]).clamp(0, 255) as u8;
                        y_plane[(py + r) * y_stride + px + c] = val;
                    }
                }
            }
        }

        // Reconstruct 4 U sub-blocks.
        reconstruct_chroma(
            &mut u_plane,
            uv_stride,
            mb_row,
            mb_col,
            &u_pred,
            &u_blocks,
            &quantizer,
        );

        // Reconstruct 4 V sub-blocks.
        reconstruct_chroma(
            &mut v_plane,
            uv_stride,
            mb_row,
            mb_col,
            &v_pred,
            &v_blocks,
            &quantizer,
        );
    }

    // Convert YUV 4:2:0 to interleaved RGB.
    let w = header.width as usize;
    let h = header.height as usize;
    let rgb = yuv420_to_rgb(&y_plane, &u_plane, &v_plane, y_stride, uv_stride, w, h);

    Ok((rgb, header.width as u32, header.height as u32))
}

// ---------------------------------------------------------------------------
// RIFF / frame header parsing
// ---------------------------------------------------------------------------

fn parse_riff_webp(data: &[u8]) -> Result<&[u8], Box<dyn Error>> {
    if data.len() < 20 {
        return Err("WebP data too short".into());
    }
    if &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return Err("not a valid WebP file".into());
    }
    if &data[12..16] != b"VP8 " {
        return Err("only lossy VP8 WebP is supported".into());
    }
    let chunk_size = u32::from_le_bytes([data[16], data[17], data[18], data[19]]) as usize;
    let end = (20 + chunk_size).min(data.len());
    Ok(&data[20..end])
}

struct FrameHeader {
    width: u16,
    height: u16,
    q_index: u8,
    prob_skip_false: u8,
}

/// Parse the 3-byte frame tag and 7-byte key-frame header, then locate the
/// first and token partitions within the VP8 data.
///
/// Also reads and applies coefficient probability updates from the first partition,
/// returning the (possibly modified) coefficient probability table.
#[allow(clippy::type_complexity)]
fn parse_frame_header(
    vp8: &[u8],
) -> Result<(FrameHeader, &[u8], &[u8], [[[[u8; 11]; 3]; 8]; 4]), Box<dyn Error>> {
    if vp8.len() < 10 {
        return Err("VP8 data too short".into());
    }

    let tag = u32::from(vp8[0]) | (u32::from(vp8[1]) << 8) | (u32::from(vp8[2]) << 16);
    let is_keyframe = (tag & 1) == 0;
    if !is_keyframe {
        return Err("not a VP8 key frame".into());
    }
    let first_part_size = (tag >> 5) as usize;

    // Key-frame start code.
    if vp8[3] != 0x9D || vp8[4] != 0x01 || vp8[5] != 0x2A {
        return Err("invalid VP8 start code".into());
    }
    let width = u16::from_le_bytes([vp8[6], vp8[7]]) & 0x3FFF;
    let height = u16::from_le_bytes([vp8[8], vp8[9]]) & 0x3FFF;

    let first_start = 10;
    let first_end = first_start + first_part_size;
    if first_end > vp8.len() {
        return Err("first partition extends past end of data".into());
    }
    let first_part = &vp8[first_start..first_end];
    let token_part = &vp8[first_end..];

    // Parse the header fields from the first partition to extract q_index.
    let mut dec = BoolDecoder::new(first_part);

    // color_space, clamping
    let _color_space = dec.read_bit(128);
    let _clamping = dec.read_bit(128);

    // segmentation
    let seg_enabled = dec.read_bit(128);
    if seg_enabled {
        return Err("VP8 segmentation not supported".into());
    }

    // loop filter
    let _filter_type = dec.read_bit(128);
    let _filter_level = dec.read_literal(6);
    let _sharpness = dec.read_literal(3);

    // loop-filter deltas
    let lf_delta_enabled = dec.read_bit(128);
    if lf_delta_enabled {
        let lf_delta_update = dec.read_bit(128);
        if lf_delta_update {
            for _ in 0..4 {
                if dec.read_bit(128) {
                    dec.read_signed(6);
                }
            }
            for _ in 0..4 {
                if dec.read_bit(128) {
                    dec.read_signed(6);
                }
            }
        }
    }

    // log2_nbr_of_dct_partitions
    let _log2_parts = dec.read_literal(2);

    // Quantization index
    let q_index = dec.read_literal(7) as u8;
    // All five delta-present flags: our encoder always writes false.
    let y_dc_present = dec.read_bit(128);
    if y_dc_present {
        dec.read_signed(4);
    }
    let y2_dc_present = dec.read_bit(128);
    if y2_dc_present {
        dec.read_signed(4);
    }
    let y2_ac_present = dec.read_bit(128);
    if y2_ac_present {
        dec.read_signed(4);
    }
    let uv_dc_present = dec.read_bit(128);
    if uv_dc_present {
        dec.read_signed(4);
    }
    let uv_ac_present = dec.read_bit(128);
    if uv_ac_present {
        dec.read_signed(4);
    }

    // refresh_entropy_probs
    let _refresh = dec.read_bit(128);

    // Read coefficient probability updates (RFC 6386 Section 13.4)
    let mut coeff_probs = DEFAULT_COEFF_PROBS;
    for t in 0..4 {
        for b in 0..8 {
            for c in 0..3 {
                for n in 0..11 {
                    let update = dec.read_bit(COEFF_UPDATE_PROBS[t][b][c][n]);
                    if update {
                        coeff_probs[t][b][c][n] = dec.read_literal(8) as u8;
                    }
                }
            }
        }
    }

    // mb_no_skip_coeff
    let mb_no_skip = dec.read_bit(128);
    let prob_skip_false = if mb_no_skip {
        dec.read_literal(8) as u8
    } else {
        0
    };

    Ok((
        FrameHeader {
            width,
            height,
            q_index,
            prob_skip_false,
        },
        first_part,
        token_part,
        coeff_probs,
    ))
}

// ---------------------------------------------------------------------------
// First partition: MB modes and skip flags
// ---------------------------------------------------------------------------

/// Re-decode the first partition to extract per-MB skip flags, Y modes, and
/// UV modes.  We re-create a fresh `BoolDecoder` because the header parsing
/// already consumed the decoder state above.
#[allow(clippy::type_complexity)]
fn decode_first_partition(
    first_part: &[u8],
    mb_count: usize,
    prob_skip_false: u8,
) -> Result<(Vec<bool>, Vec<u8>, Vec<u8>), Box<dyn Error>> {
    let mut dec = BoolDecoder::new(first_part);

    // Skip past the header fields (same sequence as parse_frame_header).
    // color_space, clamping
    dec.read_bit(128);
    dec.read_bit(128);
    // segmentation
    let seg = dec.read_bit(128);
    if seg {
        return Err("segmentation not supported".into());
    }
    // filter
    dec.read_bit(128);
    dec.read_literal(6);
    dec.read_literal(3);
    // lf delta
    let lf_delta = dec.read_bit(128);
    if lf_delta {
        let lf_update = dec.read_bit(128);
        if lf_update {
            for _ in 0..4 {
                if dec.read_bit(128) {
                    dec.read_signed(6);
                }
            }
            for _ in 0..4 {
                if dec.read_bit(128) {
                    dec.read_signed(6);
                }
            }
        }
    }
    // log2 partitions
    dec.read_literal(2);
    // quant
    dec.read_literal(7);
    for _ in 0..5 {
        if dec.read_bit(128) {
            dec.read_signed(4);
        }
    }
    // refresh
    dec.read_bit(128);

    // Read coefficient probability updates (must match parse_frame_header)
    for probs_t in &COEFF_UPDATE_PROBS {
        for probs_b in probs_t {
            for probs_c in probs_b {
                for &prob_n in probs_c {
                    let update = dec.read_bit(prob_n);
                    if update {
                        dec.read_literal(8);
                    }
                }
            }
        }
    }

    // mb_no_skip + prob_skip_false
    let mb_no_skip = dec.read_bit(128);
    if mb_no_skip {
        dec.read_literal(8);
    }

    // Now read the per-macroblock data.
    let mut skip_flags = Vec::with_capacity(mb_count);
    let mut y_modes = Vec::with_capacity(mb_count);
    let mut uv_modes = Vec::with_capacity(mb_count);

    for _ in 0..mb_count {
        let skip = if mb_no_skip {
            dec.read_bit(prob_skip_false)
        } else {
            false
        };
        skip_flags.push(skip);
        y_modes.push(decode_y_mode(&mut dec));
        uv_modes.push(decode_uv_mode(&mut dec));
    }

    Ok((skip_flags, y_modes, uv_modes))
}

// ---------------------------------------------------------------------------
// Mode trees (inverse of encode_y_mode / encode_uv_mode in bitstream.rs)
// ---------------------------------------------------------------------------

/// Decode luma 16x16 mode from the keyframe Y mode tree.
///
/// KEYFRAME_YMODE_TREE = [-B_PRED, 2, 4, 6, -DC, -V, -H, -TM]
/// Node 0 (prob 145): false=B_PRED(4), true -> Node 1
/// Node 1 (prob 156): false -> Node 2, true -> Node 3
/// Node 2 (prob 163): false=DC(0), true=V(1)
/// Node 3 (prob 128): false=H(2), true=TM(3)
fn decode_y_mode(dec: &mut BoolDecoder) -> u8 {
    if !dec.read_bit(145) {
        return 4; // B_PRED (not used by our encoder)
    }
    if !dec.read_bit(156) {
        // Node 2
        if !dec.read_bit(163) {
            return 0; // DC
        }
        return 1; // V
    }
    // Node 3
    if !dec.read_bit(128) {
        return 2; // H
    }
    3 // TM
}

/// Decode chroma mode from the keyframe UV mode tree.
///
/// KEYFRAME_UV_MODE_TREE = [-DC, 2, -V, 4, -H, -TM]
/// Node 0 (prob 142): false=DC(0), true -> Node 1
/// Node 1 (prob 114): false=V(1), true -> Node 2
/// Node 2 (prob 183): false=H(2), true=TM(3)
fn decode_uv_mode(dec: &mut BoolDecoder) -> u8 {
    if !dec.read_bit(142) {
        return 0; // DC
    }
    if !dec.read_bit(114) {
        return 1; // V
    }
    if !dec.read_bit(183) {
        2 // H
    } else {
        3 // TM
    }
}

// ---------------------------------------------------------------------------
// VP8 coefficient block decoding using the standard token tree
// ---------------------------------------------------------------------------

/// Decode a single 4x4 coefficient block using the VP8 token tree.
///
/// `coeff_type`: 0=Y(AC), 1=Y2, 2=UV, 3=Y(full/B_PRED)
/// `first_coeff`: index of the first coefficient in zigzag order (0 for most, 1 for Y AC)
/// `init_ctx`: initial context from inter-block complexity (0, 1, or 2)
///
/// Returns `(coefficients, has_nonzero)`.
fn decode_coefficient_block_vp8(
    dec: &mut BoolDecoder,
    coeff_probs: &[[[[u8; 11]; 3]; 8]; 4],
    coeff_type: usize,
    first_coeff: usize,
    init_ctx: usize,
) -> ([i16; 16], bool) {
    let mut coeffs = [0i16; 16];

    // Context from inter-block complexity
    let mut ctx: usize = init_ctx;
    // After a ZERO token, skip the EOB node for the next coefficient
    let mut skip_eob = false;
    let mut has_nonzero = false;

    for scan_pos in first_coeff..16 {
        let band = COEFF_BANDS[scan_pos];
        let probs = &coeff_probs[coeff_type][band][ctx];

        if !skip_eob {
            // Node 0: EOB vs more coefficients
            if !dec.read_bit(probs[0]) {
                break; // EOB
            }
        }

        // Node 1: ZERO vs nonzero
        if !dec.read_bit(probs[1]) {
            ctx = 0; // zero: context becomes 0
            skip_eob = true;
            continue;
        }

        // Non-zero: decode magnitude via token tree
        let abs_val = decode_token_tree_vp8(dec, probs);

        // Sign bit (always at prob=128)
        let sign = dec.read_bit(128);
        let coeff_idx = ZIGZAG[scan_pos];
        coeffs[coeff_idx] = if sign {
            -(abs_val as i16)
        } else {
            abs_val as i16
        };

        has_nonzero = true;
        ctx = if abs_val == 1 { 1 } else { 2 };
        skip_eob = false;
    }

    (coeffs, has_nonzero)
}

/// Decode the magnitude of a non-zero coefficient from the VP8 token tree.
///
/// VP8 DCT token tree (RFC 6386 Section 13.2):
///   Node 2 (prob[2]): false=ONE(1), true -> Node 3
///   Node 3 (prob[3]): false -> Node 4 (TWO-FOUR), true -> Node 6 (CAT1-6)
///   Node 4 (prob[4]): false=TWO(2), true -> Node 5
///   Node 5 (prob[5]): false=THREE(3), true=FOUR(4)
///   Node 6 (prob[6]): false -> Node 7 (CAT1-2), true -> Node 8 (CAT3-6)
///   Node 7 (prob[7]): false=CAT1(5+extra), true=CAT2(7+extra)
///   Node 8 (prob[8]): false -> Node 9 (CAT3-4), true -> Node 10 (CAT5-6)
///   Node 9 (prob[9]): false=CAT3(11+extra), true=CAT4(19+extra)
///   Node 10 (prob[10]): false=CAT5(35+extra), true=CAT6(67+extra)
fn decode_token_tree_vp8(dec: &mut BoolDecoder, probs: &[u8; 11]) -> u32 {
    // Node 2: ONE vs higher
    if !dec.read_bit(probs[2]) {
        return 1; // ONE
    }

    // Node 3: TWO-FOUR (false) vs CAT1-6 (true)
    if !dec.read_bit(probs[3]) {
        // Node 4: TWO vs THREE-FOUR
        if !dec.read_bit(probs[4]) {
            return 2; // TWO
        }
        // Node 5: THREE vs FOUR
        if !dec.read_bit(probs[5]) {
            return 3; // THREE
        }
        return 4; // FOUR
    }

    // Node 6: CAT1-2 (false) vs CAT3-6 (true)
    if !dec.read_bit(probs[6]) {
        // Node 7: CAT1 vs CAT2
        if !dec.read_bit(probs[7]) {
            // CAT1: base=5, 1 extra bit
            let extra = dec.read_bit(CAT1_PROB[0]) as u32;
            return 5 + extra;
        }
        // CAT2: base=7, 2 extra bits
        let mut extra = 0u32;
        extra = (extra << 1) | dec.read_bit(CAT2_PROB[0]) as u32;
        extra = (extra << 1) | dec.read_bit(CAT2_PROB[1]) as u32;
        return 7 + extra;
    }

    // Node 8: CAT3-4 (false) vs CAT5-6 (true)
    if !dec.read_bit(probs[8]) {
        // Node 9: CAT3 vs CAT4
        if !dec.read_bit(probs[9]) {
            // CAT3: base=11, 3 extra bits
            let mut extra = 0u32;
            for &p in &CAT3_PROB {
                extra = (extra << 1) | dec.read_bit(p) as u32;
            }
            return 11 + extra;
        }
        // CAT4: base=19, 4 extra bits
        let mut extra = 0u32;
        for &p in &CAT4_PROB {
            extra = (extra << 1) | dec.read_bit(p) as u32;
        }
        return 19 + extra;
    }

    // Node 10: CAT5 vs CAT6
    if !dec.read_bit(probs[10]) {
        // CAT5: base=35, 5 extra bits
        let mut extra = 0u32;
        for &p in &CAT5_PROB {
            extra = (extra << 1) | dec.read_bit(p) as u32;
        }
        return 35 + extra;
    }

    // CAT6: base=67, 11 extra bits
    let mut extra = 0u32;
    for &p in &CAT6_PROB {
        extra = (extra << 1) | dec.read_bit(p) as u32;
    }
    67 + extra
}

// ---------------------------------------------------------------------------
// Inverse transforms
// ---------------------------------------------------------------------------

/// Inverse 4x4 DCT (VP8 specification, Sections 14.3-14.4).
///
/// Matches the reference `vp8_short_idct4x4llm_c`: column pass first, row pass
/// second (with the `+4` rounding bias). See the encoder-side `transform::idct4x4`
/// for why the pass order is load-bearing.
fn idct4x4(input: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];

    // Column pass.
    for i in 0..4 {
        let c0 = input[i];
        let c1 = input[i + 4];
        let c2 = input[i + 8];
        let c3 = input[i + 12];

        let a1 = c0 + c2;
        let b1 = c0 - c2;
        let temp1 = ((c1 * 35468) >> 16) - c3 - ((c3 * 20091) >> 16);
        let temp2 = c1 + ((c1 * 20091) >> 16) + ((c3 * 35468) >> 16);

        tmp[i] = a1 + temp2;
        tmp[i + 12] = a1 - temp2;
        tmp[i + 4] = b1 + temp1;
        tmp[i + 8] = b1 - temp1;
    }

    // Row pass (with +4 rounding bias, then >>3 normalization).
    let mut result = [0i32; 16];
    for i in 0..4 {
        let c0 = tmp[i * 4];
        let c1 = tmp[i * 4 + 1];
        let c2 = tmp[i * 4 + 2];
        let c3 = tmp[i * 4 + 3];

        let a1 = c0 + c2;
        let b1 = c0 - c2;
        let temp1 = ((c1 * 35468) >> 16) - c3 - ((c3 * 20091) >> 16);
        let temp2 = c1 + ((c1 * 20091) >> 16) + ((c3 * 35468) >> 16);

        result[i * 4] = (a1 + temp2 + 4) >> 3;
        result[i * 4 + 3] = (a1 - temp2 + 4) >> 3;
        result[i * 4 + 1] = (b1 + temp1 + 4) >> 3;
        result[i * 4 + 2] = (b1 - temp1 + 4) >> 3;
    }
    result
}

/// Inverse 4x4 Walsh-Hadamard transform for Y2 DC coefficients.
fn iwht4x4(input: &[i16; 16]) -> [i16; 16] {
    let mut tmp = [0i32; 16];

    // Row pass.
    for i in 0..4 {
        let a = input[i * 4] as i32;
        let b = input[i * 4 + 1] as i32;
        let c = input[i * 4 + 2] as i32;
        let d = input[i * 4 + 3] as i32;
        tmp[i * 4] = a + b + c + d;
        tmp[i * 4 + 1] = a + b - c - d;
        tmp[i * 4 + 2] = a - b - c + d;
        tmp[i * 4 + 3] = a - b + c - d;
    }

    // Column pass with >>3 normalization.
    let mut result = [0i16; 16];
    for i in 0..4 {
        let a = tmp[i];
        let b = tmp[i + 4];
        let c = tmp[i + 8];
        let d = tmp[i + 12];
        result[i] = ((a + b + c + d + 3) >> 3) as i16;
        result[i + 4] = ((a + b - c - d + 3) >> 3) as i16;
        result[i + 8] = ((a - b - c + d + 3) >> 3) as i16;
        result[i + 12] = ((a - b + c - d + 3) >> 3) as i16;
    }
    result
}

/// Dequantize a Y2 block with separate DC/AC factors, then apply the
/// inverse WHT.  Returns the 16 reconstructed DC values for the Y
/// sub-blocks.
fn iwht4x4_dequant_dc_ac(block: &[i16; 16], dc_quant: i16, ac_quant: i16) -> [i16; 16] {
    let mut dq = [0i16; 16];
    dq[0] = block[0].saturating_mul(dc_quant);
    for i in 1..16 {
        dq[i] = block[i].saturating_mul(ac_quant);
    }
    iwht4x4(&dq)
}

// ---------------------------------------------------------------------------
// Plane reconstruction helpers
// ---------------------------------------------------------------------------

fn gather_borders_16(
    plane: &[u8],
    stride: usize,
    mb_row: usize,
    mb_col: usize,
) -> ([u8; 16], [u8; 16], u8) {
    let mut above = [128u8; 16];
    let mut left = [128u8; 16];
    let mut tl = 128u8;
    let by = mb_row * 16;
    let bx = mb_col * 16;
    if mb_row > 0 {
        let row = by - 1;
        for c in 0..16 {
            above[c] = plane[row * stride + bx + c];
        }
    }
    if mb_col > 0 {
        for r in 0..16 {
            left[r] = plane[(by + r) * stride + bx - 1];
        }
    }
    if mb_row > 0 && mb_col > 0 {
        tl = plane[(by - 1) * stride + bx - 1];
    }
    (above, left, tl)
}

fn gather_borders_8(
    plane: &[u8],
    stride: usize,
    mb_row: usize,
    mb_col: usize,
) -> ([u8; 8], [u8; 8], u8) {
    let mut above = [128u8; 8];
    let mut left = [128u8; 8];
    let mut tl = 128u8;
    let by = mb_row * 8;
    let bx = mb_col * 8;
    if mb_row > 0 {
        let row = by - 1;
        for c in 0..8 {
            above[c] = plane[row * stride + bx + c];
        }
    }
    if mb_col > 0 {
        for r in 0..8 {
            left[r] = plane[(by + r) * stride + bx - 1];
        }
    }
    if mb_row > 0 && mb_col > 0 {
        tl = plane[(by - 1) * stride + bx - 1];
    }
    (above, left, tl)
}

fn write_pred_y(plane: &mut [u8], stride: usize, mb_row: usize, mb_col: usize, pred: &[u8; 256]) {
    let by = mb_row * 16;
    let bx = mb_col * 16;
    for r in 0..16 {
        let off = (by + r) * stride + bx;
        plane[off..off + 16].copy_from_slice(&pred[r * 16..(r + 1) * 16]);
    }
}

fn write_pred_uv(plane: &mut [u8], stride: usize, mb_row: usize, mb_col: usize, pred: &[u8; 64]) {
    let by = mb_row * 8;
    let bx = mb_col * 8;
    for r in 0..8 {
        let off = (by + r) * stride + bx;
        plane[off..off + 8].copy_from_slice(&pred[r * 8..(r + 1) * 8]);
    }
}

fn reconstruct_chroma(
    plane: &mut [u8],
    stride: usize,
    mb_row: usize,
    mb_col: usize,
    pred: &[u8; 64],
    blocks: &[[i16; 16]; 4],
    quantizer: &VpxQuantizer,
) {
    for by in 0..2 {
        for bx in 0..2 {
            let blk_idx = by * 2 + bx;
            let mut dequant = [0i32; 16];
            dequant[0] = blocks[blk_idx][0] as i32 * quantizer.uv_dc as i32;
            for i in 1..16 {
                dequant[i] = blocks[blk_idx][i] as i32 * quantizer.uv_ac as i32;
            }
            let pixels = idct4x4(&dequant);

            let py = mb_row * 8 + by * 4;
            let px = mb_col * 8 + bx * 4;
            for r in 0..4 {
                for c in 0..4 {
                    let pred_val = pred[(by * 4 + r) * 8 + bx * 4 + c] as i32;
                    let val = (pred_val + pixels[r * 4 + c]).clamp(0, 255) as u8;
                    plane[(py + r) * stride + px + c] = val;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// YUV 4:2:0 -> RGB conversion
// ---------------------------------------------------------------------------

/// Convert YUV 4:2:0 planes to interleaved RGB using the inverse of the
/// BT.601 matrix used by the encoder in encode.rs.
fn yuv420_to_rgb(
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    y_stride: usize,
    uv_stride: usize,
    w: usize,
    h: usize,
) -> Vec<u8> {
    let mut rgb = vec![0u8; w * h * 3];

    for row in 0..h {
        for col in 0..w {
            let y_val = y_plane[row * y_stride + col] as i32;
            let u_val = u_plane[(row / 2) * uv_stride + col / 2] as i32;
            let v_val = v_plane[(row / 2) * uv_stride + col / 2] as i32;

            let c = 298 * (y_val - 16);
            let d = u_val - 128;
            let e = v_val - 128;

            let r = ((c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
            let g = ((c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
            let b = ((c + 516 * d + 128) >> 8).clamp(0, 255) as u8;

            let idx = (row * w + col) * 3;
            rgb[idx] = r;
            rgb[idx + 1] = g;
            rgb[idx + 2] = b;
        }
    }
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::webp::encode_webp;

    /// Helper: compute Peak Signal-to-Noise Ratio between two same-length
    /// RGB buffers.
    fn psnr(a: &[u8], b: &[u8]) -> f64 {
        let mse: f64 = a
            .iter()
            .zip(b.iter())
            .map(|(&x, &y)| {
                let d = x as f64 - y as f64;
                d * d
            })
            .sum::<f64>()
            / a.len() as f64;
        if mse < 0.001 {
            return 100.0;
        }
        10.0 * (255.0_f64 * 255.0 / mse).log10()
    }

    #[test]
    fn decode_roundtrip_solid_color() {
        let (w, h) = (16, 16);
        let rgb = vec![128u8; w * h * 3];
        let webp = encode_webp(&rgb, w as u32, h as u32, 80).unwrap();
        let (decoded, dw, dh) = decode_webp(&webp).unwrap();
        assert_eq!(dw, w as u32);
        assert_eq!(dh, h as u32);
        assert_eq!(decoded.len(), w * h * 3);
        let p = psnr(&rgb, &decoded);
        assert!(p > 15.0, "PSNR {p:.1} dB too low for solid-color roundtrip");
    }

    #[test]
    fn decode_roundtrip_gradient() {
        let (w, h) = (32, 32);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let idx = (y * w + x) * 3;
                rgb[idx] = (x * 255 / (w - 1)) as u8;
                rgb[idx + 1] = (y * 255 / (h - 1)) as u8;
                rgb[idx + 2] = 128;
            }
        }
        let webp = encode_webp(&rgb, w as u32, h as u32, 90).unwrap();
        let (decoded, dw, dh) = decode_webp(&webp).unwrap();
        assert_eq!(dw, w as u32);
        assert_eq!(dh, h as u32);
        let p = psnr(&rgb, &decoded);
        assert!(p > 10.0, "PSNR {p:.1} dB too low for gradient roundtrip");
    }

    #[test]
    fn decode_roundtrip_1x1() {
        let rgb = vec![255u8, 0, 0];
        let webp = encode_webp(&rgb, 1, 1, 75).unwrap();
        let (decoded, dw, dh) = decode_webp(&webp).unwrap();
        assert_eq!(dw, 1);
        assert_eq!(dh, 1);
        assert_eq!(decoded.len(), 3);
    }

    #[test]
    fn decode_roundtrip_non_multiple_of_16() {
        let (w, h) = (13u32, 7u32);
        let rgb = vec![100u8; (w * h * 3) as usize];
        let webp = encode_webp(&rgb, w, h, 75).unwrap();
        let (decoded, dw, dh) = decode_webp(&webp).unwrap();
        assert_eq!(dw, w);
        assert_eq!(dh, h);
        assert_eq!(decoded.len(), (w * h * 3) as usize);
    }

    #[test]
    fn decode_rejects_non_webp() {
        let bad = b"not a webp file at all";
        assert!(decode_webp(bad).is_err());
    }

    #[test]
    fn decode_rejects_truncated() {
        let rgb = vec![128u8; 16 * 16 * 3];
        let webp = encode_webp(&rgb, 16, 16, 75).unwrap();
        // Truncate to just the RIFF header.
        assert!(decode_webp(&webp[..12]).is_err());
    }
}
