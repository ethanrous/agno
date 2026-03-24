//! GPU-accelerated demosaic implementation using wgpu.
//!
//! The shader is compiled from Rust to SPIR-V at build time using rust-gpu.
//! The SPIR-V binary is loaded directly by wgpu.

use crate::demosaic::BayerPattern;
use crate::gpu::{
    GpuContext, GpuPipeline, create_output_buffer, create_storage_buffer, create_uniform_buffer,
    dispatch_and_read, workgroups_2d,
};
use crate::sony_decoder::Dimensions;
use agno_gpu_shared::DemosaicParams;

/// Load the SPIR-V shader (compiled at build time)
const GPU_KERNELS_SPV: &[u8] = include_bytes!(env!("GPU_KERNELS_SPV_PATH"));

/// Attempt GPU-accelerated bilinear demosaic.
/// Returns None if GPU is unavailable, allowing fallback to CPU.
pub fn demosaic_bilinear_to_rgb8_gpu(
    raw: &[u16],
    dims: Dimensions,
    pattern: BayerPattern,
    black_level: u16,
    white_level: u16,
    wb: [f32; 3],
    color_matrix: [f32; 9],
    gamma: f32,
) -> Option<Vec<u8>> {
    let ctx = GpuContext::get()?;

    let w = dims.output_width as u32;
    let h = dims.output_height as u32;
    let stride = dims.raw_width as u32;

    let pattern_u32 = match pattern {
        BayerPattern::RGGB => 0,
        BayerPattern::BGGR => 1,
    };

    let params = DemosaicParams::new(
        w,
        h,
        stride,
        pattern_u32,
        black_level as u32,
        white_level as u32,
        wb,
        color_matrix,
        gamma,
    );

    // Create compute pipeline
    let pipeline = GpuPipeline::new(ctx, GPU_KERNELS_SPV, "demosaic_kernel", "demosaic");

    // Create buffers
    let params_buffer = create_uniform_buffer(ctx, &params, "demosaic-params");

    // Convert raw u16 to u32 for GPU (u16 isn't universally supported in compute)
    let raw_u32: Vec<u32> = raw.iter().map(|&v| v as u32).collect();
    let input_buffer = create_storage_buffer(ctx, &raw_u32, "demosaic-input");

    // Create output buffer (one u32 per pixel for packed RGB)
    let output_size = (w * h * 4) as u64;
    let output_buffer = create_output_buffer(ctx, output_size, "demosaic-output");

    // Create bind group
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("demosaic-bind-group"),
        layout: &pipeline.bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: input_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: output_buffer.as_entire_binding(),
            },
        ],
    });

    // Dispatch and read results
    let workgroups = workgroups_2d(w, h);
    let packed_output: Vec<u32> = dispatch_and_read(
        ctx,
        &pipeline,
        &bind_group,
        workgroups,
        &output_buffer,
        output_size,
        "demosaic",
    )?;

    // Unpack u32 (0x00RRGGBB) to [R, G, B] bytes
    let mut rgb8 = Vec::with_capacity((w * h * 3) as usize);
    for &packed in packed_output.iter().take((w * h) as usize) {
        rgb8.push(((packed >> 16) & 0xFF) as u8); // R
        rgb8.push(((packed >> 8) & 0xFF) as u8); // G
        rgb8.push((packed & 0xFF) as u8); // B
    }

    Some(rgb8)
}
