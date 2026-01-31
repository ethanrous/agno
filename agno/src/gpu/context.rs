//! GPU context management for wgpu.
//!
//! Provides a lazy-initialized singleton GPU context with device and queue.

use std::sync::OnceLock;
use wgpu::{Adapter, Device, Queue};

/// Global GPU context singleton.
static GPU_CONTEXT: OnceLock<Option<GpuContext>> = OnceLock::new();

/// Holds the wgpu device and queue for GPU compute operations.
pub struct GpuContext {
    pub device: Device,
    pub queue: Queue,
    #[allow(dead_code)]
    pub adapter: Adapter,
}

impl GpuContext {
    /// Get the global GPU context, initializing it if necessary.
    /// Returns None if GPU is unavailable.
    pub fn get() -> Option<&'static GpuContext> {
        GPU_CONTEXT
            .get_or_init(|| pollster::block_on(async { Self::init_async().await }))
            .as_ref()
    }

    /// Initialize the GPU context asynchronously.
    async fn init_async() -> Option<GpuContext> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // Request a high-performance adapter
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await?;

        log::info!(
            "GPU adapter: {} ({:?})",
            adapter.get_info().name,
            adapter.get_info().backend
        );

        // Request device - no special features needed since we use WGSL
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("agno-gpu"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::Performance,
                },
                None,
            )
            .await
            .ok()?;

        Some(GpuContext {
            device,
            queue,
            adapter,
        })
    }
}
