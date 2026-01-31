//! GPU-accelerated demosaic implementation using wgpu.
//!
//! The shader is compiled from Rust to SPIR-V at build time using rust-gpu,
//! then converted to WGSL via naga for cross-platform compatibility (Vulkan, Metal, DX12).

use crate::demosaic::BayerPattern;
use crate::gpu::GpuContext;
use crate::sony_decoder::Dimensions;
use agno_gpu_shared::DemosaicParams;
use wgpu::util::DeviceExt;

/// Load the WGSL shader (converted from SPIR-V at build time)
const DEMOSAIC_WGSL: &str = include_str!(env!("DEMOSAIC_WGSL_PATH"));

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

    // Create shader module from WGSL
    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("demosaic-kernel"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(DEMOSAIC_WGSL)),
        });

    // Create compute pipeline
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("demosaic-pipeline"),
            layout: None, // Auto-derive from shader
            module: &shader,
            entry_point: Some("demosaic_kernel"),
            compilation_options: Default::default(),
            cache: None,
        });

    // Create uniform buffer for params
    let params_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("params-buffer"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

    // Convert raw u16 to u32 for GPU (u16 isn't universally supported in compute)
    let raw_u32: Vec<u32> = raw.iter().map(|&v| v as u32).collect();

    // Create input buffer
    let input_buffer = ctx
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("raw-input-buffer"),
            contents: bytemuck::cast_slice(&raw_u32),
            usage: wgpu::BufferUsages::STORAGE,
        });

    // Create output buffer (one u32 per pixel for packed RGB)
    let output_size = (w * h * 4) as u64; // 4 bytes per pixel (u32)
    let output_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rgb-output-buffer"),
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    // Create staging buffer for reading results
    let staging_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging-buffer"),
        size: output_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Create bind group
    let bind_group_layout = pipeline.get_bind_group_layout(0);
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("demosaic-bind-group"),
        layout: &bind_group_layout,
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

    // Dispatch compute
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("demosaic-encoder"),
        });

    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("demosaic-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        // Workgroup size is 16x16 in shader
        let workgroups_x = (w + 15) / 16;
        let workgroups_y = (h + 15) / 16;
        pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
    }

    // Copy output to staging buffer
    encoder.copy_buffer_to_buffer(&output_buffer, 0, &staging_buffer, 0, output_size);

    // Submit and wait
    ctx.queue.submit(std::iter::once(encoder.finish()));

    // Map staging buffer and read results
    let buffer_slice = staging_buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        sender.send(result).unwrap();
    });

    ctx.device.poll(wgpu::Maintain::Wait);

    if receiver.recv().ok()?.is_err() {
        return None;
    }

    // Read the packed u32 values and convert to RGB8
    let data = buffer_slice.get_mapped_range();
    let packed_output: &[u32] = bytemuck::cast_slice(&data);

    // Unpack u32 (0x00RRGGBB) to [R, G, B] bytes
    let mut rgb8 = Vec::with_capacity((w * h * 3) as usize);
    for &packed in packed_output.iter().take((w * h) as usize) {
        rgb8.push(((packed >> 16) & 0xFF) as u8); // R
        rgb8.push(((packed >> 8) & 0xFF) as u8); // G
        rgb8.push((packed & 0xFF) as u8); // B
    }

    drop(data);
    staging_buffer.unmap();

    Some(rgb8)
}
