# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Agno is a Rust image processing library with GPU acceleration. Supports JPEG, PNG, WebP, HEIC, and RAW camera files (Sony ARW, Canon CR2). Used as a Rust crate or static library (`libagno.a`) with C FFI for Go integration in the Weblens project.

## Build & Test

```bash
# Build (GPU enabled by default)
cargo build -p agno

# Build without GPU (CPU-only)
cargo build -p agno --no-default-features

# Build release static library
./build/sh/build-agno.bash /path/to/output/libagno.a

# Run tests
cargo test -p agno
cargo test -p agno --test encoder_tests

# Run CLI
cargo run -p agno -- exif <file>
cargo run -p agno -- convert <input> <output>

# Lint
cargo clippy -p agno
```

## Workspace Structure

Three crates in a Cargo workspace:

- **agno** — Main crate: format decoding/encoding, EXIF, image transforms, C FFI. Produces lib + staticlib + binary.
- **agno-gpu-shared** — Shared types with identical layout on CPU and GPU (SPIR-V). Dual `no_std`/`std`.
- **agno-gpu-kernels** — GPU compute kernels (Rust → SPIR-V via rust-gpu at build time).

## Key Architecture Decisions

- **GPU-first with CPU fallback**: All GPU operations return `Option` — caller always provides CPU fallback path. GPU unavailability is normal (Docker, CI), not an error.
- **Native codecs**: JPEG, WebP, PNG, and HEVC decoders/encoders are implemented from scratch in `codec/` (no C library dependencies). This keeps the static library self-contained.
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
| `pdf` | no | PDF rendering via pdfium (incomplete) |

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
