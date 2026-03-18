use super::predict::{
    best_16x16_mode, best_chroma_mode, predict_16x16, predict_chroma_8x8,
};
use super::quantize::{quantize_block, quantize_block_uniform, quality_to_qindex, VpxQuantizer};
use super::transform::{fdct4x4, fwht4x4};

/// YUV 4:2:0 image data with separate planes.
pub struct Yuv420 {
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub y_stride: usize,
    pub uv_stride: usize,
    pub width: u32,
    pub height: u32,
}

/// Convert interleaved RGB (3 bytes per pixel, row-major) to YUV 4:2:0 (BT.601).
///
/// The Y plane is padded to a multiple of 16 in both dimensions.
/// The U/V planes are padded to a multiple of 8.
pub fn rgb_to_yuv420(rgb: &[u8], width: u32, height: u32) -> Yuv420 {
    let w = width as usize;
    let h = height as usize;
    let y_stride = (w + 15) & !15; // round up to multiple of 16
    let y_height = (h + 15) & !15;
    let uv_stride = ((w + 1) / 2 + 7) & !7; // round up half-width to multiple of 8
    let uv_height = ((h + 1) / 2 + 7) & !7;

    let mut y_plane = vec![0u8; y_stride * y_height];
    let mut u_plane = vec![128u8; uv_stride * uv_height];
    let mut v_plane = vec![128u8; uv_stride * uv_height];

    // Compute full-resolution Y
    for row in 0..h {
        for col in 0..w {
            let idx = (row * w + col) * 3;
            let r = rgb[idx] as i32;
            let g = rgb[idx + 1] as i32;
            let b = rgb[idx + 2] as i32;
            y_plane[row * y_stride + col] =
                ((66 * r + 129 * g + 25 * b + 128) >> 8).wrapping_add(16).clamp(0, 255) as u8;
        }
    }

    // Compute subsampled U, V by averaging 2x2 pixel groups
    let uv_w = (w + 1) / 2;
    let uv_h = (h + 1) / 2;
    for row in 0..uv_h {
        for col in 0..uv_w {
            let mut r_sum = 0i32;
            let mut g_sum = 0i32;
            let mut b_sum = 0i32;
            let mut count = 0i32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let sr = row * 2 + dy;
                    let sc = col * 2 + dx;
                    if sr < h && sc < w {
                        let idx = (sr * w + sc) * 3;
                        r_sum += rgb[idx] as i32;
                        g_sum += rgb[idx + 1] as i32;
                        b_sum += rgb[idx + 2] as i32;
                        count += 1;
                    }
                }
            }
            let count = count.max(1);
            let r = r_sum / count;
            let g = g_sum / count;
            let b = b_sum / count;
            let u_val = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
            let v_val = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
            u_plane[row * uv_stride + col] = u_val.clamp(0, 255) as u8;
            v_plane[row * uv_stride + col] = v_val.clamp(0, 255) as u8;
        }
    }

    Yuv420 {
        y: y_plane,
        u: u_plane,
        v: v_plane,
        y_stride,
        uv_stride,
        width,
        height,
    }
}

/// Encode RGB pixel data as a lossy WebP image.
///
/// * `rgb` - Interleaved RGB, 3 bytes per pixel, `width * height * 3` bytes total.
/// * `width`, `height` - Image dimensions in pixels.
/// * `quality` - 0 (worst) to 100 (best).
pub fn encode_webp(
    rgb: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    #[cfg(feature = "gpu")]
    {
        if width >= 256 && height >= 256 {
            if let Some(result) = crate::webp_gpu::encode_webp_gpu(rgb, width, height, quality) {
                return Ok(result);
            }
        }
    }
    encode_webp_cpu(rgb, width, height, quality)
}

/// CPU-only WebP encoding pipeline.
pub fn encode_webp_cpu(
    rgb: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if width == 0 || height == 0 {
        return Err("image dimensions must be non-zero".into());
    }
    let expected = (width as usize) * (height as usize) * 3;
    if rgb.len() < expected {
        return Err(format!(
            "RGB buffer too small: expected {} bytes, got {}",
            expected,
            rgb.len()
        )
        .into());
    }

    let yuv = rgb_to_yuv420(rgb, width, height);
    encode_webp_from_yuv(&yuv, quality)
}

/// Encode from pre-converted YUV 4:2:0 data.
///
/// This is the core encoding pipeline shared by CPU and (future) GPU paths.
pub fn encode_webp_from_yuv(
    yuv: &Yuv420,
    quality: u8,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let width = yuv.width;
    let height = yuv.height;
    let mb_cols = ((width + 15) / 16) as usize;
    let mb_rows = ((height + 15) / 16) as usize;
    let mb_count = mb_cols * mb_rows;

    let q_index = quality_to_qindex(quality);
    let quantizer = VpxQuantizer::new(q_index);

    let mut mb_modes_y = Vec::with_capacity(mb_count);
    let mut mb_modes_uv = Vec::with_capacity(mb_count);
    let mut all_coeffs: Vec<Vec<[i16; 16]>> = Vec::with_capacity(mb_count);
    let mut skip_flags = Vec::with_capacity(mb_count);

    for mb_row in 0..mb_rows {
        for mb_col in 0..mb_cols {
            // --- Luma (Y) prediction and transform ---
            let (y_above, y_left, y_tl) =
                get_y_borders(&yuv.y, yuv.y_stride, mb_row, mb_col);

            let y_mode = best_16x16_mode(
                &yuv.y,
                yuv.y_stride,
                mb_row,
                mb_col,
                &y_above,
                &y_left,
                y_tl,
            );
            let has_above = mb_row > 0;
            let has_left = mb_col > 0;
            let y_pred =
                predict_16x16(&y_above, &y_left, y_tl, y_mode, has_above, has_left);

            // --- Chroma (U, V) prediction and transform ---
            let (u_above, u_left, u_tl) =
                get_uv_borders(&yuv.u, yuv.uv_stride, mb_row, mb_col);
            let (v_above, v_left, v_tl) =
                get_uv_borders(&yuv.v, yuv.uv_stride, mb_row, mb_col);

            let uv_mode = best_chroma_mode(
                &yuv.u,
                yuv.uv_stride,
                mb_row,
                mb_col,
                &u_above,
                &u_left,
                u_tl,
            );

            let u_pred =
                predict_chroma_8x8(&u_above, &u_left, u_tl, uv_mode, has_above, has_left);
            let v_pred =
                predict_chroma_8x8(&v_above, &v_left, v_tl, uv_mode, has_above, has_left);

            // Compute residuals, DCT, and quantize for all blocks.
            let mut mb_blocks = Vec::with_capacity(25);

            // Process 16 Y sub-blocks (4x4 each) and collect DC coefficients for Y2.
            let mut y_dc_values = [0i16; 16];
            let mut y_blocks = [[0i16; 16]; 16];

            for sub_row in 0..4 {
                for sub_col in 0..4 {
                    let block_idx = sub_row * 4 + sub_col;
                    let residual = compute_y_residual(
                        &yuv.y,
                        yuv.y_stride,
                        mb_row,
                        mb_col,
                        sub_row,
                        sub_col,
                        &y_pred,
                    );
                    let dct = fdct4x4(&residual);
                    let quantized = quantize_block(&dct, quantizer.y1_dc, quantizer.y1_ac);
                    y_dc_values[block_idx] = quantized[0];
                    // Zero out DC for the Y block (it goes into Y2)
                    let mut y_block = quantized;
                    y_block[0] = 0;
                    y_blocks[block_idx] = y_block;
                }
            }

            // Y2 block: WHT of the 16 DC coefficients
            let y2_wht = fwht4x4(&y_dc_values);
            let y2_quantized =
                quantize_block_uniform(&y2_wht, quantizer.y2_dc.max(quantizer.y2_ac));

            // Block order: Y2, then 16 Y, then 4 U, then 4 V
            mb_blocks.push(y2_quantized);
            for block in &y_blocks {
                mb_blocks.push(*block);
            }

            // 4 U sub-blocks
            for sub_row in 0..2 {
                for sub_col in 0..2 {
                    let residual = compute_uv_residual(
                        &yuv.u,
                        yuv.uv_stride,
                        mb_row,
                        mb_col,
                        sub_row,
                        sub_col,
                        &u_pred,
                    );
                    let dct = fdct4x4(&residual);
                    let quantized = quantize_block(&dct, quantizer.uv_dc, quantizer.uv_ac);
                    mb_blocks.push(quantized);
                }
            }

            // 4 V sub-blocks
            for sub_row in 0..2 {
                for sub_col in 0..2 {
                    let residual = compute_uv_residual(
                        &yuv.v,
                        yuv.uv_stride,
                        mb_row,
                        mb_col,
                        sub_row,
                        sub_col,
                        &v_pred,
                    );
                    let dct = fdct4x4(&residual);
                    let quantized = quantize_block(&dct, quantizer.uv_dc, quantizer.uv_ac);
                    mb_blocks.push(quantized);
                }
            }

            // Check if all coefficients in this macroblock are zero.
            let is_skip = mb_blocks.iter().all(|b| b.iter().all(|&c| c == 0));

            mb_modes_y.push(y_mode);
            mb_modes_uv.push(uv_mode);
            all_coeffs.push(mb_blocks);
            skip_flags.push(is_skip);
        }
    }

    let vp8_data = super::bitstream::encode_vp8_frame(
        width,
        height,
        &quantizer,
        &mb_modes_y,
        &mb_modes_uv,
        &all_coeffs,
        &skip_flags,
    );

    Ok(super::riff::wrap_riff_webp(&vp8_data))
}

/// Extract the border pixels above and to the left of a 16x16 luma macroblock.
fn get_y_borders(
    y_plane: &[u8],
    stride: usize,
    mb_row: usize,
    mb_col: usize,
) -> ([u8; 16], [u8; 16], u8) {
    let mut above = [128u8; 16];
    let mut left = [128u8; 16];
    let mut top_left = 128u8;

    let base_y = mb_row * 16;
    let base_x = mb_col * 16;

    if mb_row > 0 {
        let row_above = base_y - 1;
        for c in 0..16 {
            above[c] = y_plane[row_above * stride + base_x + c];
        }
    }

    if mb_col > 0 {
        for r in 0..16 {
            left[r] = y_plane[(base_y + r) * stride + base_x - 1];
        }
    }

    if mb_row > 0 && mb_col > 0 {
        top_left = y_plane[(base_y - 1) * stride + base_x - 1];
    }

    (above, left, top_left)
}

/// Extract the border pixels above and to the left of an 8x8 chroma macroblock.
fn get_uv_borders(
    uv_plane: &[u8],
    stride: usize,
    mb_row: usize,
    mb_col: usize,
) -> ([u8; 8], [u8; 8], u8) {
    let mut above = [128u8; 8];
    let mut left = [128u8; 8];
    let mut top_left = 128u8;

    let base_y = mb_row * 8;
    let base_x = mb_col * 8;

    if mb_row > 0 {
        let row_above = base_y - 1;
        for c in 0..8 {
            above[c] = uv_plane[row_above * stride + base_x + c];
        }
    }

    if mb_col > 0 {
        for r in 0..8 {
            left[r] = uv_plane[(base_y + r) * stride + base_x - 1];
        }
    }

    if mb_row > 0 && mb_col > 0 {
        top_left = uv_plane[(base_y - 1) * stride + base_x - 1];
    }

    (above, left, top_left)
}

/// Compute the 4x4 residual block for a Y sub-block within a macroblock.
///
/// `sub_row`, `sub_col`: which of the 4x4 sub-blocks (0..3 each).
/// `pred_16x16`: the full 16x16 prediction for this macroblock.
fn compute_y_residual(
    y_plane: &[u8],
    stride: usize,
    mb_row: usize,
    mb_col: usize,
    sub_row: usize,
    sub_col: usize,
    pred_16x16: &[u8; 256],
) -> [i16; 16] {
    let mut residual = [0i16; 16];
    let base_y = mb_row * 16 + sub_row * 4;
    let base_x = mb_col * 16 + sub_col * 4;
    let pred_base_y = sub_row * 4;
    let pred_base_x = sub_col * 4;

    for r in 0..4 {
        for c in 0..4 {
            let orig = y_plane[(base_y + r) * stride + base_x + c] as i16;
            let pred = pred_16x16[(pred_base_y + r) * 16 + pred_base_x + c] as i16;
            residual[r * 4 + c] = orig - pred;
        }
    }
    residual
}

/// Compute the 4x4 residual block for a U or V sub-block within a chroma macroblock.
///
/// `sub_row`, `sub_col`: which of the 4x4 sub-blocks (0..1 each within the 8x8 block).
/// `pred_8x8`: the full 8x8 chroma prediction for this macroblock.
fn compute_uv_residual(
    uv_plane: &[u8],
    stride: usize,
    mb_row: usize,
    mb_col: usize,
    sub_row: usize,
    sub_col: usize,
    pred_8x8: &[u8; 64],
) -> [i16; 16] {
    let mut residual = [0i16; 16];
    let base_y = mb_row * 8 + sub_row * 4;
    let base_x = mb_col * 8 + sub_col * 4;
    let pred_base_y = sub_row * 4;
    let pred_base_x = sub_col * 4;

    for r in 0..4 {
        for c in 0..4 {
            let orig = uv_plane[(base_y + r) * stride + base_x + c] as i16;
            let pred = pred_8x8[(pred_base_y + r) * 8 + pred_base_x + c] as i16;
            residual[r * 4 + c] = orig - pred;
        }
    }
    residual
}
