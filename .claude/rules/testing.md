# Testing

## Running Tests

```bash
# All agno tests
just test

# Specific test by name
just test jpeg_roundtrip

# Release mode (matches production behavior, catches debug-only issues)
just test --release

# With output (for debugging — pass --nocapture)
just test --nocapture
```

Raw `cargo test -p agno` also works. See `justfile` for the exact invocations.

## Test Locations

| Location | What it tests |
|----------|---------------|
| `agno/tests/encoder_tests.rs` | JPEG/WebP roundtrip encoding, quality, markers |
| Inline `#[cfg(test)] mod tests` | Unit tests within modules (load/heic.rs, load/load.rs, load/mov.rs) |

## Test Data

Test images live in `tests/data/`:
- `sony.ARW`, `sony2.ARW` — Sony RAW files
- `cannon.CR2` — Canon RAW file (note: typo "cannon" is historical, keep it)
- `sideways.jpeg`, `sideways2.heic` — Images with EXIF rotation
- `test-heic.heic` — HEIC still image
- `sample.mov` — QuickTime MOV with embedded thumbnail
- `cannon.pgm`, `cannon.tiff` — Reference outputs for comparison

## Writing Tests

### Codec Tests

Codec tests validate encode/decode quality and correctness:

```rust
#[test]
fn jpeg_roundtrip_quality() {
    let input = create_test_image(64, 64);  // Known RGB8 data
    let encoded = encode_jpeg(&input, 64, 64, 90).unwrap();
    let (decoded, w, h) = decode_jpeg(&encoded).unwrap();
    assert_eq!((w, h), (64, 64));
    let psnr = calculate_psnr(&input, &decoded);
    assert!(psnr > 15.0, "PSNR too low: {psnr}");
}
```

Key validation patterns:
- **PSNR** for lossy codecs (> 15 dB at quality 90)
- **Exact equality** for lossless roundtrips
- **File size monotonicity**: higher quality → larger output
- **Format markers**: verify correct headers (SOI/EOI for JPEG, RIFF for WebP)

### Loader Tests

Loader tests use real files from `tests/data/`:

```rust
#[test]
fn load_sony_raw() {
    let img = load_agno_image_from_file("tests/data/sony.ARW").unwrap();
    assert!(img.width > 0 && img.height > 0);
    assert_eq!(img.len(), (img.width * img.height * 3) as usize);
}
```

### GPU Tests

GPU tests must handle the no-GPU case:

```rust
#[test]
fn gpu_resize_or_skip() {
    if gpu::GpuContext::get().is_none() {
        eprintln!("No GPU available, skipping");
        return;
    }
    // ... GPU-specific assertions
}
```

## What NOT to Test

- Constant values or table contents (quantization tables, Huffman tables)
- Private helper functions in isolation (test through public API)
- GPU availability itself (environment-dependent)
- Exact byte output of lossy encoders (implementation-dependent)

## What MUST Be Tested

- Every new codec: roundtrip encode/decode
- Every new loader: loads test file, produces valid dimensions
- Every new FFI function: called from Rust test simulating C caller
- Edge cases: zero-dimension images, empty files, truncated data
