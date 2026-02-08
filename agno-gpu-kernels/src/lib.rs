//! GPU compute kernels for agno image processing.
//!
//! This crate is compiled to SPIR-V using rust-gpu and loaded by the main agno crate.
//! All code here must be `no_std` compatible when targeting SPIR-V.

#![cfg_attr(target_arch = "spirv", no_std)]

use agno_gpu_shared::{
    apply_saturation, apply_tone_curve, cfa_color_at, clamp_i32, clamp_u8, lanczos_weight,
    max_f32, pow_f32, DemosaicParams, ResizeParams, Vec4, CFA_B, CFA_G, CFA_R,
};
use spirv_std::glam::UVec3;
use spirv_std::spirv;

/// Saturating subtraction for u32 (SPIR-V doesn't have this builtin)
#[inline]
fn saturating_sub_u32(a: u32, b: u32) -> u32 {
    if a > b {
        a - b
    } else {
        0
    }
}

/// Calculate array index from row, col, and stride
#[inline]
fn idx(row: u32, col: u32, stride: u32) -> usize {
    (row * stride + col) as usize
}

/// Sample a raw pixel, apply black level subtraction, normalization, and white balance.
#[inline]
fn sample_wb(
    raw_input: &[u32],
    row: u32,
    col: u32,
    params: &DemosaicParams,
) -> f32 {
    let raw_val = raw_input[idx(row, col, params.stride)];
    let v = saturating_sub_u32(raw_val, params.black_level) as f32 * params.inv_range;
    let color = cfa_color_at(row, col, params.pattern);
    let gain = match color {
        CFA_R => params.wb.x,
        CFA_G => params.wb.y,
        _ => params.wb.z,
    };
    v * gain
}

/// Convert linear value to gamma-corrected u32 with tone curve (for GPU pixel packing)
#[inline]
fn tone_u32(v: f32, gamma: f32) -> u32 {
    let curved = apply_tone_curve(v);
    let gamma_safe = if gamma < 0.001 { 0.001 } else { gamma };
    let corrected = pow_f32(curved, 1.0 / gamma_safe) * 255.0 + 0.5;
    if corrected < 0.0 {
        0
    } else if corrected > 255.0 {
        255
    } else {
        corrected as u32
    }
}

/// Apply 3x3 color correction matrix using separate row Vec4s
#[inline]
fn apply_color_matrix(
    r: f32,
    g: f32,
    b: f32,
    row0: &Vec4,
    row1: &Vec4,
    row2: &Vec4,
) -> (f32, f32, f32) {
    (
        row0.x * r + row0.y * g + row0.z * b,
        row1.x * r + row1.y * g + row1.z * b,
        row2.x * r + row2.y * g + row2.z * b,
    )
}

/// Bilinear demosaic compute kernel.
///
/// Each thread processes one output pixel.
/// Input: raw Bayer mosaic data (u16 packed as u32)
/// Output: RGB8 pixels (packed as u32: 0x00RRGGBB)
#[spirv(compute(threads(16, 16)))]
pub fn demosaic_kernel(
    #[spirv(global_invocation_id)] id: UVec3,
    #[spirv(uniform, descriptor_set = 0, binding = 0)] params: &DemosaicParams,
    #[spirv(storage_buffer, descriptor_set = 0, binding = 1)] raw_input: &[u32],
    #[spirv(storage_buffer, descriptor_set = 0, binding = 2)] rgb_output: &mut [u32],
) {
    let x = id.x;
    let y = id.y;

    // Bounds check
    if x >= params.width || y >= params.height {
        return;
    }

    let w = params.width;
    let h = params.height;

    // Clamp neighbor coordinates
    let x0 = clamp_i32(x as i32 - 1, 0, (w - 1) as i32) as u32;
    let x2 = clamp_i32(x as i32 + 1, 0, (w - 1) as i32) as u32;
    let y0 = clamp_i32(y as i32 - 1, 0, (h - 1) as i32) as u32;
    let y2 = clamp_i32(y as i32 + 1, 0, (h - 1) as i32) as u32;

    // Sample neighbors with white balance
    let here = sample_wb(raw_input, y, x, params);
    let up = sample_wb(raw_input, y0, x, params);
    let down = sample_wb(raw_input, y2, x, params);
    let left = sample_wb(raw_input, y, x0, params);
    let right = sample_wb(raw_input, y, x2, params);

    // Bilinear interpolation based on CFA pattern
    let color = cfa_color_at(y, x, params.pattern);
    let (mut r, mut g, mut b) = if color == CFA_R {
        // Red pixel: have R, need G and B
        let ul = sample_wb(raw_input, y0, x0, params);
        let ur = sample_wb(raw_input, y0, x2, params);
        let dl = sample_wb(raw_input, y2, x0, params);
        let dr = sample_wb(raw_input, y2, x2, params);
        let g_interp = (up + down + left + right) * 0.25;
        let b_interp = (ul + ur + dl + dr) * 0.25;
        (here, g_interp, b_interp)
    } else if color == CFA_B {
        // Blue pixel: have B, need R and G
        let ul = sample_wb(raw_input, y0, x0, params);
        let ur = sample_wb(raw_input, y0, x2, params);
        let dl = sample_wb(raw_input, y2, x0, params);
        let dr = sample_wb(raw_input, y2, x2, params);
        let g_interp = (up + down + left + right) * 0.25;
        let r_interp = (ul + ur + dl + dr) * 0.25;
        (r_interp, g_interp, here)
    } else {
        // Green pixel: have G, need R and B
        // Decide which color lies horizontally
        let horiz_color = cfa_color_at(y, x ^ 1, params.pattern);
        let h_avg = (left + right) * 0.5;
        let v_avg = (up + down) * 0.5;
        let (r_val, b_val) = if horiz_color == CFA_R {
            (h_avg, v_avg)
        } else {
            (v_avg, h_avg)
        };
        (r_val, here, b_val)
    };

    // Clamp negatives
    r = max_f32(r, 0.0);
    g = max_f32(g, 0.0);
    b = max_f32(b, 0.0);

    // Apply color correction matrix
    let (r_cc, g_cc, b_cc) = apply_color_matrix(
        r,
        g,
        b,
        &params.color_matrix_r0,
        &params.color_matrix_r1,
        &params.color_matrix_r2,
    );
    r = max_f32(r_cc, 0.0);
    g = max_f32(g_cc, 0.0);
    b = max_f32(b_cc, 0.0);

    // Apply exposure compensation
    r = r * params.exposure_mult;
    g = g * params.exposure_mult;
    b = b * params.exposure_mult;

    // Apply saturation boost
    let (r_sat, g_sat, b_sat) = apply_saturation(r, g, b, params.saturation);

    // Convert to u8 with tone curve and gamma
    let r_u8 = tone_u32(r_sat, params.gamma);
    let g_u8 = tone_u32(g_sat, params.gamma);
    let b_u8 = tone_u32(b_sat, params.gamma);

    // Pack RGB into output (we'll store 3 bytes per pixel, but use u32 for alignment)
    let out_idx = (y * w + x) as usize;
    rgb_output[out_idx] = (r_u8 << 16) | (g_u8 << 8) | b_u8;
}

/// Horizontal resize pass (src_width -> dst_width, height unchanged)
/// Input: packed RGB pixels (0x00RRGGBB)
/// Output: horizontally resized packed RGB pixels
#[spirv(compute(threads(16, 16)))]
pub fn resize_horizontal_kernel(
    #[spirv(global_invocation_id)] id: UVec3,
    #[spirv(uniform, descriptor_set = 0, binding = 0)] params: &ResizeParams,
    #[spirv(storage_buffer, descriptor_set = 0, binding = 1)] input: &[u32],
    #[spirv(storage_buffer, descriptor_set = 0, binding = 2)] output: &mut [u32],
) {
    let dst_x = id.x;
    let dst_y = id.y;

    // Bounds check
    if dst_x >= params.dst_width || dst_y >= params.src_height {
        return;
    }

    // Map output coord to input coord (center of filter)
    let src_x_f = (dst_x as f32 + 0.5) * params.scale_x - 0.5;
    let src_x_center = src_x_f as i32;
    let radius = params.filter_radius as i32;

    let mut sum_r = 0.0f32;
    let mut sum_g = 0.0f32;
    let mut sum_b = 0.0f32;
    let mut weight_sum = 0.0f32;

    // Sample pixels within filter radius
    let mut i = -radius;
    while i <= radius {
        let src_x = clamp_i32(src_x_center + i, 0, (params.src_width - 1) as i32) as u32;
        let dist = (src_x_center + i) as f32 - src_x_f;
        let weight = lanczos_weight(dist, params.filter_radius);

        let pixel = input[(dst_y * params.src_width + src_x) as usize];
        sum_r += ((pixel >> 16) & 0xFF) as f32 * weight;
        sum_g += ((pixel >> 8) & 0xFF) as f32 * weight;
        sum_b += (pixel & 0xFF) as f32 * weight;
        weight_sum += weight;

        i += 1;
    }

    // Normalize and clamp
    let inv = 1.0 / max_f32(weight_sum, 0.0001);
    let r = clamp_u8((sum_r * inv + 0.5) as i32);
    let g = clamp_u8((sum_g * inv + 0.5) as i32);
    let b = clamp_u8((sum_b * inv + 0.5) as i32);

    output[(dst_y * params.dst_width + dst_x) as usize] = (r << 16) | (g << 8) | b;
}

/// Vertical resize pass (height: src_height -> dst_height, width unchanged)
/// Input: horizontally resized packed RGB pixels (0x00RRGGBB)
/// Output: fully resized packed RGB pixels
#[spirv(compute(threads(16, 16)))]
pub fn resize_vertical_kernel(
    #[spirv(global_invocation_id)] id: UVec3,
    #[spirv(uniform, descriptor_set = 0, binding = 0)] params: &ResizeParams,
    #[spirv(storage_buffer, descriptor_set = 0, binding = 1)] input: &[u32],
    #[spirv(storage_buffer, descriptor_set = 0, binding = 2)] output: &mut [u32],
) {
    let dst_x = id.x;
    let dst_y = id.y;

    // Bounds check
    if dst_x >= params.dst_width || dst_y >= params.dst_height {
        return;
    }

    // Map output coord to input coord (center of filter)
    let src_y_f = (dst_y as f32 + 0.5) * params.scale_y - 0.5;
    let src_y_center = src_y_f as i32;
    let radius = params.filter_radius as i32;

    let mut sum_r = 0.0f32;
    let mut sum_g = 0.0f32;
    let mut sum_b = 0.0f32;
    let mut weight_sum = 0.0f32;

    // Sample pixels within filter radius
    let mut i = -radius;
    while i <= radius {
        let src_y = clamp_i32(src_y_center + i, 0, (params.src_height - 1) as i32) as u32;
        let dist = (src_y_center + i) as f32 - src_y_f;
        let weight = lanczos_weight(dist, params.filter_radius);

        // Input width is dst_width (from horizontal pass)
        let pixel = input[(src_y * params.dst_width + dst_x) as usize];
        sum_r += ((pixel >> 16) & 0xFF) as f32 * weight;
        sum_g += ((pixel >> 8) & 0xFF) as f32 * weight;
        sum_b += (pixel & 0xFF) as f32 * weight;
        weight_sum += weight;

        i += 1;
    }

    // Normalize and clamp
    let inv = 1.0 / max_f32(weight_sum, 0.0001);
    let r = clamp_u8((sum_r * inv + 0.5) as i32);
    let g = clamp_u8((sum_g * inv + 0.5) as i32);
    let b = clamp_u8((sum_b * inv + 0.5) as i32);

    output[(dst_y * params.dst_width + dst_x) as usize] = (r << 16) | (g << 8) | b;
}
