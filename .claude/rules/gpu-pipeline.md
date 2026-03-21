# GPU Pipeline

## Build Pipeline

When `gpu` feature is enabled (default):

1. **`agno/build.rs`** invokes `spirv-builder` targeting `spirv-unknown-vulkan1.1`
2. **`agno-gpu-kernels`** is compiled to a `.spv` SPIR-V binary
3. Binary path exported as `GPU_KERNELS_SPV_PATH` env var
4. **`agno`** embeds it via `include_bytes!(env!("GPU_KERNELS_SPV_PATH"))`
5. At runtime, wgpu loads the embedded SPIR-V

The nightly toolchain in `rust-toolchain.toml` is required for rust-gpu's SPIR-V backend. Changing this version can break GPU compilation.

## Runtime Context

`gpu/context.rs` provides a singleton `GpuContext`:

```rust
static GPU_CONTEXT: OnceLock<Option<GpuContext>> = OnceLock::new();
```

- Initialized lazily on first `GpuContext::get()` call
- Returns `None` if no Vulkan-compatible GPU found (Docker, CI, some laptops)
- Rejects CPU/software adapters — CPU fallback code is faster than CPU shaders
- Thread-safe via `OnceLock`

## GPU Fallback Pattern

**Every GPU operation returns `Option`, never `Result`:**

```rust
// GPU module — returns None if GPU unavailable or operation fails
pub fn resize_gpu(data: &[u8], ...) -> Option<Vec<u8>> {
    let ctx = GpuContext::get()?;  // None if no GPU
    // ... GPU implementation ...
    Some(result)
}

// Caller in transform.rs — always provides CPU fallback
#[cfg(feature = "gpu")]
if let Some(resized) = resize_gpu(data, ...) {
    return Ok(AgnoImage::new(resized, new_w, new_h, exif));
}
// CPU fallback (runs unconditionally when GPU absent or feature disabled)
let resized = ops::resize_lanczos3(data, ...);
```

**Why Option, not Result:** GPU unavailability is expected (CI, Docker, Intel iGPUs without Vulkan). It's not an error — it's a fast path that may not exist. The caller always has a CPU path.

## Compute Dispatch

`gpu/pipeline.rs` provides helpers:

1. `GpuPipeline::new(ctx, spv, entry_point, label)` — compiles SPIR-V bytes + entry point to compute pipeline
2. `create_uniform_buffer(params)` — uploads `#[repr(C)]` params struct
3. `create_storage_buffer(data)` — uploads input data
4. `create_output_buffer(size)` — allocates GPU output buffer
5. `dispatch_and_read(pipeline, bind_group, workgroups)` — runs compute + reads back

## Shared Types (agno-gpu-shared)

Parameter structs must be identical on CPU and GPU:

- `#[repr(C)]` for deterministic layout
- `bytemuck::Pod + Zeroable` for safe GPU buffer casting
- `#[cfg_attr(target_arch = "spirv", no_std)]` — same code compiles for both targets
- Math functions (`lanczos_weight`, `cfa_color_at`, etc.) work on both CPU and GPU

## Adding a New GPU Operation

1. **Define params struct** in `agno-gpu-shared/src/lib.rs`:
   - `#[repr(C)]`, derive `Copy, Clone, Pod, Zeroable`
   - All fields must be GPU-compatible types (f32, u32, Vec4)

2. **Write kernel** in `agno-gpu-kernels/src/lib.rs`:
   - `#[spirv(compute(threads(N)))]` entry point
   - Use `#[spirv(global_invocation_id)]` for thread indexing
   - Access buffers via `#[spirv(storage_buffer)]` parameters

3. **Write GPU wrapper** in `agno/src/<operation>_gpu.rs`:
   - Return `Option<Vec<u8>>` (None if GPU unavailable)
   - Use `GpuPipeline` helpers for dispatch
   - Feature-gate: `#[cfg(feature = "gpu")]`

4. **Write CPU fallback** in the appropriate module (e.g., `ops.rs`)

5. **Wire up caller** to try GPU first, fall back to CPU

6. **Register in `lib.rs`**:
   ```rust
   #[cfg(feature = "gpu")]
   mod <operation>_gpu;
   ```
