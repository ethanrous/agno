//! Shared GPU pipeline utilities for compute operations.
//!
//! This module provides common abstractions for creating compute pipelines,
//! managing buffers, and dispatching GPU work.

use super::GpuContext;
use wgpu::util::DeviceExt;

/// A GPU compute pipeline wrapper.
pub struct GpuPipeline {
    pub pipeline: wgpu::ComputePipeline,
}

impl GpuPipeline {
    /// Create a new compute pipeline from SPIR-V bytecode.
    pub fn new(ctx: &GpuContext, spv: &[u8], entry_point: &str, label: &str) -> Self {
        let shader = ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::util::make_spirv(spv),
            });

        let pipeline = ctx
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: None, // Auto-derive from shader
                module: &shader,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            });

        Self { pipeline }
    }

    /// Get the bind group layout for the pipeline.
    pub fn bind_group_layout(&self, index: u32) -> wgpu::BindGroupLayout {
        self.pipeline.get_bind_group_layout(index)
    }
}

/// Create a uniform buffer initialized with data.
pub fn create_uniform_buffer<T: bytemuck::Pod>(
    ctx: &GpuContext,
    data: &T,
    label: &str,
) -> wgpu::Buffer {
    ctx.device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::bytes_of(data),
            usage: wgpu::BufferUsages::UNIFORM,
        })
}

/// Create a storage buffer initialized with data.
pub fn create_storage_buffer<T: bytemuck::Pod>(
    ctx: &GpuContext,
    data: &[T],
    label: &str,
) -> wgpu::Buffer {
    ctx.device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(data),
            usage: wgpu::BufferUsages::STORAGE,
        })
}

/// Create an output storage buffer (for GPU write, CPU read).
pub fn create_output_buffer(ctx: &GpuContext, size: u64, label: &str) -> wgpu::Buffer {
    ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// Create a staging buffer for reading GPU results back to CPU.
pub fn create_staging_buffer(ctx: &GpuContext, size: u64, label: &str) -> wgpu::Buffer {
    ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Dispatch a compute pass and read back results.
///
/// Returns None if the GPU operation fails.
pub fn dispatch_and_read<T: bytemuck::Pod + Clone>(
    ctx: &GpuContext,
    pipeline: &GpuPipeline,
    bind_group: &wgpu::BindGroup,
    workgroups: (u32, u32, u32),
    output_buffer: &wgpu::Buffer,
    output_size: u64,
    label: &str,
) -> Option<Vec<T>> {
    // Create staging buffer for reading results
    let staging_buffer = create_staging_buffer(ctx, output_size, &format!("{}-staging", label));

    // Create command encoder and dispatch
    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(&format!("{}-encoder", label)),
        });

    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some(&format!("{}-pass", label)),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups(workgroups.0, workgroups.1, workgroups.2);
    }

    // Copy output to staging buffer
    encoder.copy_buffer_to_buffer(output_buffer, 0, &staging_buffer, 0, output_size);

    // Submit and wait
    ctx.queue.submit(std::iter::once(encoder.finish()));

    // Map staging buffer and read results
    let buffer_slice = staging_buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });

    ctx.device.poll(wgpu::Maintain::Wait);

    if receiver.recv().ok()?.is_err() {
        return None;
    }

    let data = buffer_slice.get_mapped_range();
    let result: Vec<T> = bytemuck::cast_slice(&data).to_vec();
    drop(data);
    staging_buffer.unmap();

    Some(result)
}

/// Calculate workgroup counts for a 2D dispatch with 16x16 workgroup size.
pub fn workgroups_2d(width: u32, height: u32) -> (u32, u32, u32) {
    ((width + 15) / 16, (height + 15) / 16, 1)
}
