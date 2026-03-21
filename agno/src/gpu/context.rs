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

        // Log available adapters for debugging
        let adapters: Vec<_> = instance.enumerate_adapters(wgpu::Backends::all());
        if adapters.is_empty() {
            tracing::warn!(
                "No GPU adapters found. On macOS ensure Metal is available; on Linux ensure Vulkan drivers are installed."
            );

            match std::env::var("NVIDIA_DRIVER_CAPABILITIES").ok() {
                Some(ref caps) if caps.contains("compute") || caps.contains("all") => {
                    tracing::warn!(
                        "No GPU adapters found, NVIDIA_DRIVER_CAPABILITIES is {:?}.",
                        caps
                    );
                }
                _ => {
                    tracing::warn!(
                        "For NVIDIA in Docker, ensure NVIDIA_DRIVER_CAPABILITIES=all is set."
                    );
                }
            }

            return None;
        }

        for (i, adapter) in adapters.iter().enumerate() {
            let info = adapter.get_info();
            tracing::debug!(
                adapter_index = i,
                name = %info.name,
                vendor = info.vendor,
                device = info.device,
                backend = ?info.backend,
                device_type = ?info.device_type,
                "Found GPU adapter"
            );
        }

        // Request a high-performance adapter
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await;

        let adapter = match adapter {
            Some(a) => a,
            None => {
                tracing::warn!("Failed to request high-performance adapter, trying any adapter");
                instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::None,
                        compatible_surface: None,
                        force_fallback_adapter: false,
                    })
                    .await?
            }
        };

        let info = adapter.get_info();

        // Reject software/CPU adapters - they're slower than our CPU fallback code
        // Common software renderers: llvmpipe, lavapipe, SwiftShader
        if info.device_type == wgpu::DeviceType::Cpu {
            tracing::warn!(
                adapter_name = %info.name,
                "Rejecting software GPU adapter (device_type=Cpu). \
                 This is likely llvmpipe/lavapipe. For NVIDIA in Docker, ensure \
                 nvidia-container-toolkit is installed and the container has GPU access. \
                 Falling back to CPU code paths."
            );
            return None;
        }

        tracing::info!(
            adapter_name = %info.name,
            backend = ?info.backend,
            device_type = ?info.device_type,
            "GPU adapter initialized"
        );

        // Request device - no special features needed since we use WGSL
        let device_result = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("agno-gpu"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::Performance,
                },
                None,
            )
            .await;

        let (device, queue) = match device_result {
            Ok(dq) => dq,
            Err(e) => {
                tracing::error!(error = %e, "Failed to create GPU device");
                return None;
            }
        };

        tracing::info!("GPU device created successfully");

        Some(GpuContext {
            device,
            queue,
            adapter,
        })
    }
}
