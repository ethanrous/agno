# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Agno is a Rust image processing library with GPU acceleration. Supports JPEG, PNG, WebP, HEIC, and RAW camera files (Sony ARW, Canon CR2). Used as a Rust crate or static library (`libagno.a`) with C FFI for Go integration in the Weblens project.

## Build & Test

Uses [`just`](https://github.com/casey/just) as the task runner (install: `cargo install just` or `brew install just`).

```bash
# Build release static library
just build                                          # default output: libagno.a
just build libagno.a --target aarch64-apple-darwin  # cross-compile
just build libagno.a --features gpu,jpeg,png,webp   # custom features

# Build via Docker (cross-compilation)
just docker-build arm64
just docker-build amd64 --pdf

# Run tests
just test                       # all tests
just test jpeg_roundtrip        # specific test
just test --release             # release mode

# Lint
just lint                       # check formatting + clippy
just lint --fix                 # auto-fix

# Format / clippy individually
just fmt
just clippy

# Fast compile check (no codegen)
just check

# Run CLI
just run exif <file>
just run convert <input> <output>

# Clean
just clean
```

Raw cargo commands also work — see `justfile` for the exact invocations.

## Workspace Structure

Three crates in a Cargo workspace:

- **agno** — Main crate: format decoding/encoding, EXIF, image transforms, C FFI. Produces lib + staticlib + binary.
- **agno-gpu-shared** — Shared types with identical layout on CPU and GPU (SPIR-V). Dual `no_std`/`std`.
- **agno-gpu-kernels** — GPU compute kernels (Rust → SPIR-V via rust-gpu at build time).

## Key Architecture Decisions

- **No runtime dynamic library dependencies**: The default build must produce a fully self-contained static library with zero `.so`/`.dylib` requirements at runtime. All C/C++ dependencies (e.g., pdfium) must be statically linked. Opt-in feature flags may offer dynamic linking as an alternative, but the default must always be self-contained. This is critical — agno produces `libagno.a` consumed via CGO, and runtime deps break deployment (Docker, cross-compilation).
- **GPU-first with CPU fallback**: All GPU operations return `Option` — caller always provides CPU fallback path. GPU unavailability is normal (Docker, CI), not an error.
- **Native codecs**: JPEG, WebP, PNG, HEIF/HEVC, and PDF decoders/encoders are all implemented in pure Rust in `codec/` (no C library dependencies). HEIC images are decoded natively via the HEIF container parser and HEVC still-image decoder. PDF rendering uses a native parser + tiny-skia rasterizer (replaced hayro/pdfium). This keeps the static library fully self-contained.
- **C FFI with dual allocators**: `AgnoImage` buffers use `libc::malloc()` for C/Go interop; `AgnoBuffer` (in-memory encoding) uses Rust's allocator. Each has its own free function. See `.claude/rules/ffi-interface.md`.
- **Format auto-detection**: `agno_image/load/load.rs` detects format by magic bytes, not file extension.
- **EXIF-driven transforms**: Orientation, white balance, color matrix all sourced from EXIF metadata.

## Feature Flags

| Flag | Default | What it enables |
|------|---------|-----------------|
| `gpu` | yes | wgpu + SPIR-V GPU acceleration |
| `jpeg` | yes | Native JPEG encode/decode |
| `png` | yes | Native PNG decode |
| `webp` | yes | Native WebP encode/decode |
| `pdf` | yes | Native PDF rasterizer (tiny-skia + ttf-parser) |
| `dicom` | yes | Native DICOM (.dcm) decode: uncompressed MONOCHROME/RGB, window/level |
| `cabac-trace` | no | Debug: log every CABAC decision for HEVC decoder comparison |

HEIF/HEVC decoding is always compiled (not feature-gated). The `heic-experimental-decoder` flag in Cargo.toml is a legacy artifact with no effect.

## Toolchain

Requires nightly Rust pinned in `rust-toolchain.toml` (for rust-gpu SPIR-V compilation). Auto-installed on first cargo command.

## Detailed Rules (auto-loaded from `.claude/rules/`)

| File | Contents |
|------|----------|
| `architecture.md` | Module map, image pipeline stages, crate relationships |
| `coding-conventions.md` | Error handling, naming, unsafe code, feature flags |
| `codec-guide.md` | Codec structure, how to add new format support |
| `hevc-decoder.md` | HEVC decoder status, CABAC bugs (fixed and remaining), debugging methodology |
| `gpu-pipeline.md` | GPU build pipeline, runtime context, fallback pattern |
| `ffi-interface.md` | C FFI conventions, memory management, adding functions |
| `testing.md` | Running tests, test data, quality validation |

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
