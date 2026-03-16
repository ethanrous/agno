# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Agno is a general-purpose Rust image processing library with GPU acceleration. It supports standard formats (JPEG, PNG, WebP) as well as RAW camera files (Sony ARW, Canon CR2). The library can be used as a Rust crate or as a static library (`libagno.a`) with C FFI for integration with other languages.

## Build Commands

```bash
# Build with GPU support (default)
cargo build -p agno --features gpu

# Build without GPU (CPU-only)
cargo build -p agno --no-default-features

# Build release static library for current platform
./build/sh/build-agno.bash /path/to/output/libagno.a

# Build shared crate only
cargo build -p agno-gpu-shared

# Run the CLI tool
cargo run -p agno -- exif <file>
cargo run -p agno -- convert <input> <output>
```

## Architecture

### Workspace Structure

The project is a Cargo workspace with three crates:

- **agno** - Main crate: RAW decoding, EXIF parsing, image transforms, C FFI. Produces both library and binary.
- **agno-gpu-shared** - Shared types/functions that must have identical layout on CPU and GPU (SPIR-V). Uses `#![cfg_attr(target_arch = "spirv", no_std)]`.
- **agno-gpu-kernels** - GPU compute kernels written in Rust, compiled to SPIR-V via rust-gpu at build time.

### GPU Pipeline

When `gpu` feature is enabled:

1. `agno/build.rs` uses `spirv-builder` to compile `agno-gpu-kernels` to SPIR-V
2. The SPIR-V binary is embedded via `include_bytes!(env!("GPU_KERNELS_SPV_PATH"))`
3. At runtime, wgpu loads the SPIR-V and dispatches compute shaders
4. All GPU operations fall back to CPU implementations if GPU is unavailable

Key GPU modules:

- `gpu/context.rs` - Singleton wgpu device/queue initialization
- `gpu/pipeline.rs` - Shared utilities for compute dispatch
- `demosaic_gpu.rs` - GPU Bayer demosaicing
- `resize_gpu.rs` - GPU Lanczos3 resize (two-pass separable filter)

### C FFI

`lib_interface.rs` exposes C functions for foreign language integration:

- `load_image_from_path()` - Load and decode image files
- `resize_image()` - Scale images (uses GPU when available)
- `write_agno_image_to_webp()` - Export to WebP
- `get_exif_value()` - Read EXIF metadata
- `free_agno_image()` - Memory cleanup
- `init_agno()` - Initialize logging

### Image Loading

`agno_image/load/` contains format-specific loaders:

- `load.rs` - Auto-detection and standard formats (JPEG, PNG, WebP via the `image` crate)
- `sony.rs` - Sony ARW RAW files
- `canon.rs` - Canon CR2 RAW files

For RAW files, the pipeline is: parse EXIF → extract RAW data → demosaic (Bayer to RGB) → apply color matrix/white balance → optional transforms.

## Toolchain Requirements

This project requires a specific nightly Rust toolchain for rust-gpu SPIR-V compilation. The version is pinned in `rust-toolchain.toml`. The toolchain will be automatically installed when you run cargo commands.

## Feature Flags

- `gpu` (default) - Enable GPU acceleration via wgpu/SPIR-V
- `pdf` - Enable PDF rendering via pdfium (currently incomplete)

## Development Workflow: Test-Driven Development (MANDATORY)

Every bug fix and feature MUST follow this sequence. Do not skip steps.

1. **Understand** — Read the relevant code. Use plan mode for non-trivial work. Identify the root cause (bug) or the exact behavior change (feature).
2. **Design the solution** — Decide what to change, but **do NOT write implementation code yet**.
3. **Write the test first** — Add a test that captures the expected behavior. Extend an existing test file whenever possible. Do not create new test files unless the domain is genuinely new.
4. **Run the test — watch it fail** — Confirm the test fails for the right reason (not a syntax error or import issue). This validates the test actually tests something.
5. **Implement the fix/feature** — Write the minimum code to make the test pass.
6. **Run the test — watch it pass** — If it fails, fix the implementation, not the test (unless the test itself was wrong, but be very sure the test is wrong, it is likely the implementation is the issue).
7. **Run the full relevant test suite.**

**Why this order matters:** Writing the test first forces you to define the expected behavior precisely before touching implementation code. It catches regressions, prevents over-engineering, and proves the fix actually works. Skipping to implementation and writing tests after is not TDD — it's rationalization.
