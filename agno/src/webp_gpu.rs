//! GPU-accelerated WebP encoding: color conversion on GPU, VP8 pipeline on CPU.

use crate::gpu::{
    GpuContext, GpuPipeline, create_output_buffer, create_storage_buffer, create_uniform_buffer,
    dispatch_and_read, workgroups_2d,
};
use agno_gpu_shared::ColorConvertParams;
use tracing::debug;

const GPU_KERNELS_SPV: &[u8] = include_bytes!(env!("GPU_KERNELS_SPV_PATH"));

/// GPU-accelerated WebP encoding.
/// Returns None if GPU unavailable.
pub fn encode_webp_gpu(rgb: &[u8], width: u32, height: u32, quality: u8) -> Option<Vec<u8>> {
    let ctx = GpuContext::get()?;

    // Guard: the color-convert storage buffer must fit within the device's
    // max_storage_buffer_binding_size (128 MB on Metal). Large images fall back to CPU.
    let max_binding = ctx.device.limits().max_storage_buffer_binding_size as u64;
    let input_bytes = width as u64 * height as u64 * 4;
    if input_bytes > max_binding {
        debug!(
            input_bytes,
            max_binding, "Image too large for GPU WebP encode storage buffer, falling back to CPU"
        );
        return None;
    }

    debug!(width, height, quality, "Starting GPU WebP encode");

    // Pack RGB to u32
    let packed: Vec<u32> = rgb
        .chunks_exact(3)
        .map(|c| ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32)
        .collect();

    // GPU color conversion
    let params = ColorConvertParams {
        width,
        height,
        _pad0: 0,
        _pad1: 0,
    };
    let pipeline = GpuPipeline::new(ctx, GPU_KERNELS_SPV, "rgb_to_yuv_kernel", "webp-color");
    let params_buf = create_uniform_buffer(ctx, &params, "webp-color-params");
    let input_buf = create_storage_buffer(ctx, &packed, "webp-color-input");
    let output_size = (width * height * 4) as u64;
    let output_buf = create_output_buffer(ctx, output_size, "webp-color-output");

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("webp-color-bind"),
        layout: &pipeline.bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: input_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: output_buf.as_entire_binding(),
            },
        ],
    });

    let workgroups = workgroups_2d(width, height);
    let yuv_packed: Vec<u32> = dispatch_and_read(
        ctx,
        &pipeline,
        &bind_group,
        workgroups,
        &output_buf,
        output_size,
        "webp-color",
    )?;

    // Unpack YUV and subsample to YUV420
    let w = width as usize;
    let h = height as usize;
    let y_stride = (w + 15) & !15;
    let y_height = (h + 15) & !15;
    let uv_stride = ((w + 1) / 2 + 7) & !7; // ceil(w/2), matching encode.rs
    let uv_height = ((h + 1) / 2 + 7) & !7; // ceil(h/2), matching encode.rs

    let mut y_plane = vec![0u8; y_stride * y_height];
    let mut u_plane = vec![128u8; uv_stride * uv_height];
    let mut v_plane = vec![128u8; uv_stride * uv_height];

    // Extract Y plane
    for row in 0..h {
        for col in 0..w {
            let idx = row * w + col;
            let packed_val = yuv_packed[idx];
            y_plane[row * y_stride + col] = ((packed_val >> 16) & 0xFF) as u8;
        }
    }

    // Subsample U,V by averaging 2x2 groups
    let uv_w = w / 2;
    let uv_h = h / 2;
    for row in 0..uv_h {
        for col in 0..uv_w {
            let mut u_sum = 0u32;
            let mut v_sum = 0u32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let idx = (row * 2 + dy) * w + (col * 2 + dx);
                    let p = yuv_packed[idx];
                    u_sum += (p >> 8) & 0xFF;
                    v_sum += p & 0xFF;
                }
            }
            u_plane[row * uv_stride + col] = ((u_sum + 2) / 4) as u8;
            v_plane[row * uv_stride + col] = ((v_sum + 2) / 4) as u8;
        }
    }

    let yuv = crate::codec::webp::encode::Yuv420 {
        y: y_plane,
        u: u_plane,
        v: v_plane,
        y_stride,
        uv_stride,
        width,
        height,
    };

    Some(crate::codec::webp::encode::encode_webp_from_yuv(&yuv, quality).ok()?)
}
