//! Shared types between CPU and GPU code for agno image processing.
//!
//! This crate defines types that must have identical memory layout on both
//! the CPU (host) and GPU (SPIR-V shader) sides.

#![cfg_attr(target_arch = "spirv", no_std)]
#![allow(unexpected_cfgs)] // "spirv" is valid when cross-compiling for rust-gpu

/// A 4-component float vector compatible with SPIR-V std140 layout.
/// This is 16-byte aligned as required by Vulkan uniform buffers.
#[repr(C, align(16))]
#[derive(Copy, Clone)]
#[cfg_attr(not(target_arch = "spirv"), derive(bytemuck::Pod, bytemuck::Zeroable))]
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
#[cfg_attr(not(target_arch = "spirv"), derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct DemosaicParams {
    // Offset 0: scalars packed together (each 4 bytes)
    pub width: u32,          // 0
    pub height: u32,         // 4
    pub stride: u32,         // 8
    pub pattern: u32,        // 12 (Bayer pattern: 0 = RGGB, 1 = BGGR)

    // Offset 16: more scalars
    pub black_level: u32,    // 16
    pub inv_range: f32,      // 20
    pub exposure_mult: f32,  // 24
    pub saturation: f32,     // 28

    // Offset 32: last scalar + padding to 48
    pub gamma: f32,          // 32
    pub _pad0: f32,          // 36
    pub _pad1: f32,          // 40
    pub _pad2: f32,          // 44

    // Offset 48: vec4 for white balance
    pub wb: Vec4,            // 48 (wb.x, wb.y, wb.z = RGB gains, wb.w = padding)

    // Offset 64: 3x3 color matrix stored as 3 vec4s
    /// Color correction matrix row 0 (r0, r1, r2, pad)
    pub color_matrix_r0: Vec4,  // 64
    /// Color correction matrix row 1 (g0, g1, g2, pad)
    pub color_matrix_r1: Vec4,  // 80
    /// Color correction matrix row 2 (b0, b1, b2, pad)
    pub color_matrix_r2: Vec4,  // 96
    // Total size: 112 bytes
}

impl DemosaicParams {
    #[cfg(not(target_arch = "spirv"))]
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
