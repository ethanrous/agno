//! GPU-accelerated JPEG encoding: color conversion on GPU, DCT/quantize/Huffman on CPU.

use crate::gpu::{
    GpuContext, GpuPipeline, create_output_buffer, create_storage_buffer, create_uniform_buffer,
    dispatch_and_read, workgroups_2d,
};
use agno_gpu_shared::ColorConvertParams;
use tracing::debug;

const GPU_KERNELS_SPV: &[u8] = include_bytes!(env!("GPU_KERNELS_SPV_PATH"));

/// GPU-accelerated JPEG encoding.
/// Returns None if GPU is unavailable, allowing CPU fallback.
pub fn encode_jpeg_gpu(rgb: &[u8], width: u32, height: u32, quality: u8) -> Option<Vec<u8>> {
    let ctx = GpuContext::get()?;
    debug!(width, height, quality, "Starting GPU JPEG encode");

    // Pack RGB bytes to u32 (0x00RRGGBB)
    let packed: Vec<u32> = rgb
        .chunks_exact(3)
        .map(|c| ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32)
        .collect();

    // RGB to YCbCr on GPU
    let color_params = ColorConvertParams {
        width,
        height,
        _pad0: 0,
        _pad1: 0,
    };

    let color_pipeline =
        GpuPipeline::new(ctx, GPU_KERNELS_SPV, "rgb_to_ycbcr_kernel", "jpeg-color");
    let color_params_buf = create_uniform_buffer(ctx, &color_params, "jpeg-color-params");
    let color_input_buf = create_storage_buffer(ctx, &packed, "jpeg-color-input");
    let color_output_size = (width * height * 4) as u64;
    let color_output_buf = create_output_buffer(ctx, color_output_size, "jpeg-color-output");

    let color_bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("jpeg-color-bind"),
        layout: &color_pipeline.bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: color_params_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: color_input_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: color_output_buf.as_entire_binding(),
            },
        ],
    });

    let workgroups = workgroups_2d(width, height);
    let ycbcr_packed: Vec<u32> = dispatch_and_read(
        ctx,
        &color_pipeline,
        &color_bind_group,
        workgroups,
        &color_output_buf,
        color_output_size,
        "jpeg-color",
    )?;

    debug!(pixels = ycbcr_packed.len(), "GPU color conversion complete");

    // Unpack YCbCr and feed into CPU JPEG pipeline
    let w = width as usize;
    let h = height as usize;
    let num_pixels = w * h;
    let mut y_plane = vec![0u8; num_pixels];
    let mut cb_plane = vec![0u8; num_pixels];
    let mut cr_plane = vec![0u8; num_pixels];

    for (i, &packed_val) in ycbcr_packed.iter().take(num_pixels).enumerate() {
        y_plane[i] = ((packed_val >> 16) & 0xFF) as u8;
        cb_plane[i] = ((packed_val >> 8) & 0xFF) as u8;
        cr_plane[i] = (packed_val & 0xFF) as u8;
    }

    Some(
        crate::codec::jpeg::encode::encode_jpeg_from_ycbcr(
            &y_plane, &cb_plane, &cr_plane, width, height, quality,
        )
        .ok()?,
    )
}
