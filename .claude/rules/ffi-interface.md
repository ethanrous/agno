# C FFI Interface

## Overview

`lib_interface.rs` exposes C functions for the Go backend (via CGO). There is no separate `.h` header file — C declarations are defined inline in Go's CGO bindings in the parent Weblens project.

## Memory Ownership Model

Two distinct allocation strategies:

### AgnoImage (libc allocator)
```
Rust: libc::malloc → populate → return *mut AgnoImage → C owns it
C:    receives pointer → uses it → calls free_agno_image() → libc::free()
```
- `AgnoImage.data` uses `libc::malloc()` (NOT Rust's allocator) so C/Go can free it
- `AgnoImage::new()` copies `Vec<u8>` data into `malloc`'d memory, then the Vec is dropped

### AgnoBuffer (Rust allocator)
```
Rust: Vec<u8> → mem::forget() → return AgnoBuffer { data, len } → C owns it
C:    receives buffer → uses it → calls free_agno_buffer() → Vec reconstructed + dropped
```
- `AgnoBuffer.data` uses the **Rust allocator** (Vec), NOT libc
- `free_agno_buffer()` reconstructs the Vec via `Vec::from_raw_parts()` and drops it
- Used by `write_agno_image_to_jpeg_buffer()` for in-memory encoding

Both types MUST be freed by the caller via their respective free functions.

## Type Requirements

All FFI-visible types must be `#[repr(C)]`:

```rust
#[repr(C)]
pub struct AgnoImage {
    data: *mut c_uchar,   // RGB8 or RGBA8 pixel data (see channels)
    len: usize,
    pub width: u64,
    pub height: u64,
    pub page_count: u64,
    pub channels: u8,     // 3 = RGB, 4 = RGBA
    pub exif: ExifContext,
}
```

- Field order matters — it must match the Go CGO declarations exactly
- Use `c_uchar`, `c_void`, `usize` — not Rust-specific types
- No `String`, `Vec`, `Option`, or enums across FFI boundary

**Breaking change (2026-04-18):** `channels: u8` was added between `page_count`
and `exif`. RGB images report `channels = 3`; RGBA images (from PNGs with
alpha) report `channels = 4`. `AgnoImage.data` has `width * height * channels`
bytes. Weblens CGO bindings must declare the new field in the same position.

## Function Conventions

```rust
#[unsafe(no_mangle)]
pub extern "C" fn function_name(args...) -> *mut ReturnType {
    ok_or_null!(internal_function(args))  // Returns null pointer on error
}
```

- `#[unsafe(no_mangle)]` — preserves symbol name for C linker
- `extern "C"` — C calling convention
- Error handling for `*mut AgnoImage` returns: `ok_or_null!()` macro converts `Result` to pointer (null = error, logs at `info` level). This macro is only used for functions returning `*mut AgnoImage`. Other functions (e.g., `write_agno_image_to_jpeg_buffer`) use inline match with format-specific error returns.
- Strings from C: `CString { data: *const u8, length: usize }` wrapper, validated as UTF-8

## Adding a New FFI Function

1. Write the Rust implementation in the appropriate module
2. Add the `extern "C"` wrapper in `lib_interface.rs`
3. Use `ok_or_null!()` for `*mut AgnoImage` returns, or inline match for other return types
4. Feature-gate if format-specific: `#[cfg(feature = "...")]`
5. Update the parent Weblens project's CGO bindings (Go side) to declare and call the new function

## Current FFI Surface

| Function | Returns | Purpose |
|----------|---------|---------|
| `init_agno()` | void | Initialize tracing/logging |
| `load_image_from_path(path, len)` | `*mut AgnoImage` | Load any supported format → AgnoImage |
| `resize_image(img, w, h)` | `*mut AgnoImage` | Scale image (GPU or CPU) → new AgnoImage |
| `write_agno_image_to_webp(path, len, img)` | void | Export to WebP file |
| `write_agno_image_to_jpeg_buffer(img, quality)` | `AgnoBuffer` | Encode to JPEG in-memory (Rust allocator) |
| `get_exif_value(img, tag_id)` | `ExifData` | Read EXIF tag |
| `get_gps_coordinates(img)` | `GpsCoordinates` | Extract GPS lat/lon from EXIF |
| `free_agno_image(img)` | void | Free AgnoImage (libc::free) |
| `free_agno_buffer(buf)` | void | Free AgnoBuffer (Rust allocator drop) |
| `load_pdf_page(path, len, page_num, max_w, max_h)` | `*mut AgnoImage` | Render specific PDF page (0-based index) |

## Safety Invariants

- Raw pointer from `load_image_from_path` is valid until `free_agno_image` is called
- `resize_image` consumes the old pointer (via `Box::from_raw`) and returns a NEW pointer — the caller must NOT free the old pointer after calling resize
- `get_exif_value` borrows from the image — returned data is valid only while image is alive
- Never pass a freed image pointer to any function
