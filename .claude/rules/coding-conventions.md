# Coding Conventions

## Error Handling

Three patterns used, depending on context:

1. **Custom error enums** for domain-specific modules:
   ```rust
   // exif/mod.rs — ExifError, sony_decoder.rs — DecodeError
   enum ExifError { Io(io::Error, Backtrace), NotExif, BadTiff, ... }
   impl std::error::Error for ExifError {}
   impl From<io::Error> for ExifError { ... }  // Automatic conversion with backtrace capture
   ```

2. **`Box<dyn Error>`** for load/transform functions and image codecs (JPEG, PNG, WebP):
   ```rust
   fn load_agno_image_from_file(path: &str) -> Result<AgnoImage, Box<dyn Error>>
   ```

3. **`anyhow::Result`** in container/video codec layer (HEIF, HEVC, ISOBMFF):
   ```rust
   fn parse_heif<R: Read + Seek>(reader: &mut R) -> anyhow::Result<HeifImage>
   ```
   Image codecs use `Box<dyn Error>`. Container/video codecs use `anyhow` for richer error context during complex multi-step parsing.

**Fallback-on-error**: EXIF parsing failure logs a warning and returns `ExifContext::default()` — a missing EXIF tag should never prevent loading an image.

## Naming

- Files: `snake_case.rs`. GPU variants: `{operation}_gpu.rs`
- Types: `PascalCase` (`AgnoImage`, `ExifContext`, `GpuContext`)
- Enums: `PascalCase` variants (`ImageType::SonyRaw`, `BayerPattern::RGGB`)
- Functions: `snake_case` (`load_agno_image_from_file`, `decode_jpeg`)
- Constants: `UPPER_SNAKE_CASE` (`EXPOSURE_EV`, `PNG_SIGNATURE`)

## Module Organization

- `mod.rs` uses `pub use submodule::ImportantType` to flatten re-exports
- Internal implementation modules are `mod` (private), not `pub mod`
- Public API is only what consumers need — loaders expose `load_*()`, codecs expose `encode_*()` / `decode_*()`
- Feature-gated modules use `#[cfg(feature = "...")]` at declaration site in `lib.rs`

## Feature Flag Patterns

```rust
// Codec support — compile out entire module
#[cfg(feature = "jpeg")]
ImageType::Jpeg => { decode_jpeg(data) }
#[cfg(not(feature = "jpeg"))]
ImageType::Jpeg => Err("JPEG support not enabled".into())

// GPU operations — try GPU, always provide CPU fallback
#[cfg(feature = "gpu")]
if let Some(result) = gpu_operation(...) { return Ok(result); }
// CPU fallback runs unconditionally after

// Combined features
#[cfg(all(feature = "gpu", feature = "jpeg"))]
mod jpeg_gpu;
```

## Unsafe Code Guidelines

Unsafe code is limited to two areas:

1. **FFI boundary** (`lib_interface.rs`, `image.rs`):
   - `libc::malloc()`/`libc::free()` for C-compatible memory
   - `copy_from_nonoverlapping()` for Vec → raw pointer transfer
   - `from_raw_parts()` to create slices from raw pointers
   - `from_utf8_unchecked()` after explicit UTF-8 validation

2. **GPU buffer operations** (`gpu/pipeline.rs`):
   - `bytemuck` casting for Pod types to GPU buffer layout

**Rules for new unsafe code:**
- Only at FFI boundaries or GPU interop — never in pure Rust logic
- Document the safety invariant in a `// SAFETY:` comment
- Prefer `bytemuck` over manual transmute
- Raw pointer lifetimes must be shorter than the allocation they reference

## Documentation Style

- Module-level: `//!` doc comment explaining purpose
- Public functions: `///` doc comment with return type description
- Inline comments: only for non-obvious logic (format quirks, math derivations)
- No comments on: variable assignments, struct fields with clear names, error paths using `?`

## Image Data Invariants

- `AgnoImage.data` is always RGB8 (3 bytes per pixel, row-major)
- Pixel at `(x, y)`: `data[(y * width + x) * 3 .. + 3]`
- Buffer allocated via `libc::malloc()`, freed via `libc::free()`
- Width/height are `u64` (for FFI alignment), actual images fit in `u32`
