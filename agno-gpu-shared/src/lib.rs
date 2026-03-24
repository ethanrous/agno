//! Shared types and functions between CPU and GPU code for agno image processing.
//!
//! This crate defines types that must have identical memory layout on both
//! the CPU (host) and GPU (SPIR-V shader) sides, as well as shared math functions
//! used by both implementations.

#![cfg_attr(target_arch = "spirv", no_std)]

// === Constants ===

/// Pi constant for filter calculations
pub const PI: f32 = 3.14159265358979323846;

/// Exposure adjustment in EV (Sony RAW is typically underexposed)
pub const EXPOSURE_EV: f32 = 1.2;

/// Saturation boost (25% increase)
pub const SATURATION: f32 = 1.25;

/// Tone curve power for shadow lifting (values < 1.0 lift shadows)
pub const TONE_CURVE_POWER: f32 = 0.85;

// === CFA Color Indices ===

pub const CFA_R: u32 = 0;
pub const CFA_G: u32 = 1;
pub const CFA_B: u32 = 2;

// === Shared Math Functions ===

/// Clamp i32 to range [lo, hi]
#[inline(always)]
pub fn clamp_i32(x: i32, lo: i32, hi: i32) -> i32 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// Power function that works on both CPU and GPU
#[inline(always)]
pub fn pow_f32(base: f32, exp: f32) -> f32 {
    #[cfg(target_arch = "spirv")]
    {
        spirv_std::num_traits::Float::powf(base, exp)
    }
    #[cfg(not(target_arch = "spirv"))]
    {
        base.powf(exp)
    }
}

/// Sine function that works on both CPU and GPU
#[inline(always)]
pub fn sin_f32(x: f32) -> f32 {
    #[cfg(target_arch = "spirv")]
    {
        spirv_std::num_traits::Float::sin(x)
    }
    #[cfg(not(target_arch = "spirv"))]
    {
        x.sin()
    }
}

/// Sinc function: sin(pi*x) / (pi*x)
#[inline(always)]
pub fn sinc(x: f32) -> f32 {
    let abs_x = if x < 0.0 { -x } else { x };
    if abs_x < 0.0001 {
        1.0
    } else {
        sin_f32(x * PI) / (x * PI)
    }
}

/// Lanczos filter weight function
/// a is the filter size (3.0 for Lanczos3)
#[inline(always)]
pub fn lanczos_weight(x: f32, a: f32) -> f32 {
    let abs_x = if x < 0.0 { -x } else { x };
    if abs_x >= a {
        0.0
    } else {
        sinc(x) * sinc(x / a)
    }
}

/// Clamp i32 to u8 range and return as u32
#[inline(always)]
pub fn clamp_u8(v: i32) -> u32 {
    if v < 0 {
        0
    } else if v > 255 {
        255
    } else {
        v as u32
    }
}

/// Max helper for f32
#[inline(always)]
pub fn max_f32(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

/// Calculate luminance using Rec. 709 coefficients
#[inline(always)]
pub fn luminance(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Apply saturation adjustment using luminance-preserving interpolation
#[inline(always)]
pub fn apply_saturation(r: f32, g: f32, b: f32, saturation: f32) -> (f32, f32, f32) {
    let lum = luminance(r, g, b);
    (
        lum + (r - lum) * saturation,
        lum + (g - lum) * saturation,
        lum + (b - lum) * saturation,
    )
}

/// Apply shadow-lifting tone curve using a power function
#[inline(always)]
pub fn apply_tone_curve(v: f32) -> f32 {
    let clamped = max_f32(v, 0.0);
    let result = pow_f32(clamped, TONE_CURVE_POWER);
    if result > 1.0 { 1.0 } else { result }
}

/// Convert linear value to gamma-corrected u8 with tone curve
#[inline(always)]
pub fn tone_u8(v: f32, gamma: f32) -> u8 {
    let curved = apply_tone_curve(v);
    let gamma_safe = if gamma < 0.001 { 0.001 } else { gamma };
    let corrected = pow_f32(curved, 1.0 / gamma_safe) * 255.0 + 0.5;
    if corrected < 0.0 {
        0
    } else if corrected > 255.0 {
        255
    } else {
        corrected as u8
    }
}

/// Convert linear value to gamma-corrected u32 with tone curve (for GPU packing)
#[inline(always)]
pub fn tone_u32(v: f32, gamma: f32) -> u32 {
    tone_u8(v, gamma) as u32
}

/// Determine CFA color at position. Returns CFA_R, CFA_G, or CFA_B.
/// pattern: 0 = RGGB, 1 = BGGR
#[inline(always)]
pub fn cfa_color_at(row: u32, col: u32, pattern: u32) -> u32 {
    let r = row & 1;
    let c = col & 1;
    if pattern == 0 {
        // RGGB
        match (r, c) {
            (0, 0) => CFA_R,
            (0, 1) => CFA_G,
            (1, 0) => CFA_G,
            _ => CFA_B,
        }
    } else {
        // BGGR
        match (r, c) {
            (0, 0) => CFA_B,
            (0, 1) => CFA_G,
            (1, 0) => CFA_G,
            _ => CFA_R,
        }
    }
}

/// Apply 3x3 color correction matrix (row-major [r0,r1,r2, g0,g1,g2, b0,b1,b2])
#[inline(always)]
pub fn apply_color_matrix_array(r: f32, g: f32, b: f32, m: &[f32; 9]) -> (f32, f32, f32) {
    (
        m[0] * r + m[1] * g + m[2] * b,
        m[3] * r + m[4] * g + m[5] * b,
        m[6] * r + m[7] * g + m[8] * b,
    )
}

/// A 4-component float vector compatible with SPIR-V std140 layout.
/// This is 16-byte aligned as required by Vulkan uniform buffers.
#[repr(C, align(16))]
#[derive(Copy, Clone)]
#[cfg_attr(feature = "std", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Vec4 {
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
}

/// Parameters for the GPU demosaic kernel.
/// This struct is shared between CPU (host) and GPU (shader) code.
/// Layout must be compatible with std140 SPIR-V layout rules.
///
/// std140 rules:
/// - Scalars (u32, f32): 4-byte aligned
/// - vec4: 16-byte aligned
/// - Structs: aligned to largest member alignment
#[repr(C, align(16))]
#[derive(Copy, Clone)]
#[cfg_attr(feature = "std", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct DemosaicParams {
    // Offset 0: scalars packed together (each 4 bytes)
    pub width: u32,   // 0
    pub height: u32,  // 4
    pub stride: u32,  // 8
    pub pattern: u32, // 12 (Bayer pattern: 0 = RGGB, 1 = BGGR)

    // Offset 16: more scalars
    pub black_level: u32,   // 16
    pub inv_range: f32,     // 20
    pub exposure_mult: f32, // 24
    pub saturation: f32,    // 28

    // Offset 32: last scalar + padding to 48
    pub gamma: f32, // 32
    pub _pad0: f32, // 36
    pub _pad1: f32, // 40
    pub _pad2: f32, // 44

    // Offset 48: vec4 for white balance
    pub wb: Vec4, // 48 (wb.x, wb.y, wb.z = RGB gains, wb.w = padding)

    // Offset 64: 3x3 color matrix stored as 3 vec4s
    /// Color correction matrix row 0 (r0, r1, r2, pad)
    pub color_matrix_r0: Vec4, // 64
    /// Color correction matrix row 1 (g0, g1, g2, pad)
    pub color_matrix_r1: Vec4, // 80
    /// Color correction matrix row 2 (b0, b1, b2, pad)
    pub color_matrix_r2: Vec4, // 96
                               // Total size: 112 bytes
}

impl DemosaicParams {
    #[cfg(feature = "std")]
    pub fn new(
        width: u32,
        height: u32,
        stride: u32,
        pattern: u32,
        black_level: u32,
        white_level: u32,
        wb: [f32; 3],
        color_matrix: [f32; 9],
        gamma: f32,
    ) -> Self {
        let range = (white_level.saturating_sub(black_level)).max(1) as f32;
        let inv_range = 1.0 / range;

        // "Camera look" defaults
        let exposure_mult = 2.0_f32.powf(1.2); // Sony RAW underexposed
        let saturation = 1.25; // 25% saturation boost

        Self {
            width,
            height,
            stride,
            pattern,
            black_level,
            inv_range,
            exposure_mult,
            saturation,
            gamma,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
            wb: Vec4::new(wb[0], wb[1], wb[2], 0.0),
            color_matrix_r0: Vec4::new(color_matrix[0], color_matrix[1], color_matrix[2], 0.0),
            color_matrix_r1: Vec4::new(color_matrix[3], color_matrix[4], color_matrix[5], 0.0),
            color_matrix_r2: Vec4::new(color_matrix[6], color_matrix[7], color_matrix[8], 0.0),
        }
    }
}

/// Parameters for the GPU resize kernel.
/// This struct is shared between CPU (host) and GPU (shader) code.
/// Layout must be compatible with std140 SPIR-V layout rules.
#[repr(C, align(16))]
#[derive(Copy, Clone)]
#[cfg_attr(feature = "std", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct ResizeParams {
    pub src_width: u32,     // 0
    pub src_height: u32,    // 4
    pub dst_width: u32,     // 8
    pub dst_height: u32,    // 12
    pub scale_x: f32,       // 16 (src_width / dst_width)
    pub scale_y: f32,       // 20 (src_height / dst_height)
    pub filter_radius: f32, // 24 (3.0 for Lanczos3)
    pub _pad0: u32,         // 28
}

impl ResizeParams {
    /// Create params for horizontal resize pass
    #[cfg(feature = "std")]
    pub fn new_horizontal(src_width: u32, src_height: u32, dst_width: u32) -> Self {
        let ratio = src_width as f32 / dst_width as f32;
        let filter_scale = if ratio > 1.0 { ratio } else { 1.0 };
        Self {
            src_width,
            src_height,
            dst_width,
            dst_height: src_height, // Height unchanged in horizontal pass
            scale_x: ratio,
            scale_y: 1.0,
            filter_radius: 3.0 * filter_scale,
            _pad0: 0,
        }
    }

    /// Create params for vertical resize pass
    #[cfg(feature = "std")]
    pub fn new_vertical(src_width: u32, src_height: u32, dst_height: u32) -> Self {
        let ratio = src_height as f32 / dst_height as f32;
        let filter_scale = if ratio > 1.0 { ratio } else { 1.0 };
        Self {
            src_width,
            src_height,
            dst_width: src_width, // Width unchanged in vertical pass
            dst_height,
            scale_x: 1.0,
            scale_y: ratio,
            filter_radius: 3.0 * filter_scale,
            _pad0: 0,
        }
    }
}

/// Parameters for the JPEG GPU DCT+quantize kernel.
#[repr(C, align(16))]
#[derive(Copy, Clone)]
#[cfg_attr(feature = "std", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct JpegDctParams {
    pub width: u32,             // 0: image width in pixels
    pub height: u32,            // 4: image height in pixels
    pub blocks_x: u32,          // 8: number of 8x8 blocks horizontally
    pub blocks_y: u32,          // 12: number of 8x8 blocks vertically
    pub component: u32,         // 16: 0=Y, 1=Cb, 2=Cr
    pub _pad0: u32,             // 20
    pub _pad1: u32,             // 24
    pub _pad2: u32,             // 28
    pub quant_table: [u32; 64], // 32-287: quantization table (scaled)
}

/// Parameters for the RGB color conversion kernel.
#[repr(C, align(16))]
#[derive(Copy, Clone)]
#[cfg_attr(feature = "std", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct ColorConvertParams {
    pub width: u32,  // 0
    pub height: u32, // 4
    pub _pad0: u32,  // 8
    pub _pad1: u32,  // 12
}
