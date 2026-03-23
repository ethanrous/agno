//! GPU-accelerated image resize implementation using wgpu.
//!
//! Uses a two-pass separable Lanczos3 filter for efficient high-quality resizing:
//! 1. Horizontal pass: src_width x src_height -> dst_width x src_height
//! 2. Vertical pass: dst_width x src_height -> dst_width x dst_height

use crate::gpu::{
    GpuContext, GpuPipeline, create_output_buffer, create_storage_buffer, create_uniform_buffer,
    dispatch_and_read, workgroups_2d,
};
use agno_gpu_shared::ResizeParams;
use std::sync::OnceLock;
use tracing::debug;

/// Load the SPIR-V shader (compiled at build time)
const GPU_KERNELS_SPV: &[u8] = include_bytes!(env!("GPU_KERNELS_SPV_PATH"));

static RESIZE_PIPELINES: OnceLock<(GpuPipeline, GpuPipeline)> = OnceLock::new();

fn get_resize_pipelines(ctx: &'static GpuContext) -> &'static (GpuPipeline, GpuPipeline) {
    RESIZE_PIPELINES.get_or_init(|| {
        let h = GpuPipeline::new(ctx, GPU_KERNELS_SPV, "resize_horizontal_kernel", "resize-h");
        let v = GpuPipeline::new(ctx, GPU_KERNELS_SPV, "resize_vertical_kernel", "resize-v");
        (h, v)
    })
}

/// GPU-accelerated image resize using Lanczos3 filter.
///
/// Takes RGB8 pixel data and resizes using a two-pass separable filter.
/// Returns None if GPU is unavailable, allowing fallback to CPU.
pub fn resize_gpu(
    rgb_input: &[u8],
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> Option<Vec<u8>> {
    let ctx = GpuContext::get()?;

    // Guard: all three storage buffers (input, horizontal output, vertical output) must fit
    // within the device's max_storage_buffer_binding_size. On Metal this is typically 128 MB,
    // which a ~33 MP+ image packed as u32 will exceed. Return None to fall back to CPU.
    let max_binding = ctx.device.limits().max_storage_buffer_binding_size as u64;
    let input_bytes = src_width as u64 * src_height as u64 * 4;
    let h_output_bytes = dst_width as u64 * src_height as u64 * 4;
    let v_output_bytes = dst_width as u64 * dst_height as u64 * 4;
    if input_bytes > max_binding || h_output_bytes > max_binding || v_output_bytes > max_binding {
        debug!(
            input_bytes,
            h_output_bytes,
            v_output_bytes,
            max_binding,
            "Image too large for GPU storage buffers, falling back to CPU"
        );
        return None;
    }

    debug!(
        src_width,
        src_height, dst_width, dst_height, "Starting GPU resize"
    );

    // Pack RGB bytes to u32 (0x00RRGGBB)
    let packed: Vec<u32> = rgb_input
        .chunks_exact(3)
        .map(|c| ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32)
        .collect();

    // === Pass 1: Horizontal resize (src_width x src_height -> dst_width x src_height) ===
    let (h_pipeline, v_pipeline) = get_resize_pipelines(ctx);
    let h_params = ResizeParams::new_horizontal(src_width, src_height, dst_width);

    let h_params_buffer = create_uniform_buffer(ctx, &h_params, "resize-h-params");
    let h_input_buffer = create_storage_buffer(ctx, &packed, "resize-h-input");

    // Output size for horizontal pass: dst_width x src_height
    let h_output_size = (dst_width * src_height * 4) as u64;
    let h_output_buffer = create_output_buffer(ctx, h_output_size, "resize-h-output");

    let h_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("resize-h-bind-group"),
        layout: &h_pipeline.bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: h_params_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: h_input_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: h_output_buffer.as_entire_binding(),
            },
        ],
    });

    let h_workgroups = workgroups_2d(dst_width, src_height);
    let intermediate: Vec<u32> = dispatch_and_read(
        ctx,
        &h_pipeline,
        &h_bind_group,
        h_workgroups,
        &h_output_buffer,
        h_output_size,
        "resize-h",
    )?;

    debug!(
        intermediate_len = intermediate.len(),
        expected = (dst_width * src_height) as usize,
        "Horizontal pass complete"
    );

    // === Pass 2: Vertical resize (dst_width x src_height -> dst_width x dst_height) ===
    let v_params = ResizeParams::new_vertical(dst_width, src_height, dst_height);

    let v_params_buffer = create_uniform_buffer(ctx, &v_params, "resize-v-params");
    let v_input_buffer = create_storage_buffer(ctx, &intermediate, "resize-v-input");

    // Output size for vertical pass: dst_width x dst_height
    let v_output_size = (dst_width * dst_height * 4) as u64;
    let v_output_buffer = create_output_buffer(ctx, v_output_size, "resize-v-output");

    let v_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("resize-v-bind-group"),
        layout: &v_pipeline.bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: v_params_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: v_input_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: v_output_buffer.as_entire_binding(),
            },
        ],
    });

    let v_workgroups = workgroups_2d(dst_width, dst_height);
    let packed_output: Vec<u32> = dispatch_and_read(
        ctx,
        &v_pipeline,
        &v_bind_group,
        v_workgroups,
        &v_output_buffer,
        v_output_size,
        "resize-v",
    )?;

    debug!(
        packed_output_len = packed_output.len(),
        expected = (dst_width * dst_height) as usize,
        "Vertical pass complete"
    );

    // Unpack u32 (0x00RRGGBB) to RGB bytes
    let rgb_output = unpack_to_rgb8(&packed_output, (dst_width * dst_height) as usize);

    Some(rgb_output)
}

/// Unpack packed u32 pixels (0x00RRGGBB) to RGB8 byte array.
fn unpack_to_rgb8(packed: &[u32], pixel_count: usize) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(pixel_count * 3);
    for &p in packed.iter().take(pixel_count) {
        rgb.push(((p >> 16) & 0xFF) as u8); // R
        rgb.push(((p >> 8) & 0xFF) as u8); // G
        rgb.push((p & 0xFF) as u8); // B
    }
    rgb
}
