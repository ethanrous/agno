# Codec Development Guide

## Codec Structure

Every codec in `agno/src/codec/` follows a consistent layout:

```
codec/<format>/
├── mod.rs          Re-exports: pub use encode::encode_<format>; pub use decode::decode_<format>;
├── decode.rs       Main decoder: fn decode_<format>(data: &[u8]) -> Result<(Vec<u8>, u32, u32)>
├── encode.rs       Main encoder: fn encode_<format>(rgb: &[u8], w: u32, h: u32, quality: u8) -> Result<Vec<u8>>
└── <helpers>.rs    Format-specific internals (DCT, Huffman, quantization, etc.)
```

The public API for each codec is exactly two functions:
- `decode_<format>(data: &[u8]) -> Result<(Vec<u8>, u32, u32)>` — returns `(rgb8_data, width, height)`
- `encode_<format>(rgb: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>>` — returns encoded bytes

Not all codecs have both (PNG is decode-only).

## Adding a New Codec

1. **Create the module** at `codec/<format>/`:
   - `mod.rs` with feature-gated submodule declarations
   - `decode.rs` and/or `encode.rs` matching the signature above
   - Helper files for format-specific internals

2. **Register in `codec/mod.rs`**:
   ```rust
   // Image codecs are feature-gated:
   #[cfg(feature = "<format>")]
   pub mod <format>;
   // Note: container/video codecs (heif, hevc, isobmff) are always compiled (not feature-gated)
   ```

3. **Add feature flag** in `agno/Cargo.toml`:
   ```toml
   [features]
   <format> = []  # or ["dep:some-dep"] if external dependency needed
   default = ["gpu", "jpeg", "png", "webp", "<format>"]
   ```

4. **Add format detection** in `agno_image/load/load.rs`:
   - Add variant to `ImageType` enum
   - Add magic byte detection in `detect_image_type()`
   - Add decode branch in `load_agno_image_from_file()`

5. **Add FFI export** if needed in `lib_interface.rs`:
   - Feature-gate with `#[cfg(feature = "<format>")]`

6. **Write tests** in `agno/tests/encoder_tests.rs` (or a new file if the domain is genuinely new):
   - Roundtrip test: encode → decode → compare PSNR
   - Quality parameter affects file size
   - Header/marker validation for the format

## Existing Codecs

| Codec | Decode | Encode | GPU Accel | Error Type |
|-------|--------|--------|-----------|------------|
| JPEG | `decode_jpeg()` | `encode_jpeg()` | DCT (`jpeg_gpu.rs`) | `Box<dyn Error>` |
| PNG | `decode_png()` | — | — | `Box<dyn Error>` |
| WebP | `decode_webp()` | `encode_webp()` | Encoding (`webp_gpu.rs`) | `Box<dyn Error>` |
| HEVC | `decode_hevc_still()` | — | — | `anyhow::Result` |
| HEIF | `parse_heif()` | — | — | `anyhow::Result` |

HEIF is a container format (parses to tile bitstreams), HEVC is the actual image decoder. They work together: `parse_heif()` → `decode_hevc_still()` per tile → stitch grid.

**HEVC decoder is under active development** — see `hevc-decoder.md` for detailed status, known bugs, and debugging methodology.

## Quality Validation for Codecs

Encoder tests use PSNR (Peak Signal-to-Noise Ratio) to validate quality:
- Quality 90: PSNR > 15 dB (lossy formats)
- Higher quality → larger file size (monotonic)
- Roundtrip: encode(decode(file)) should produce reasonable PSNR

For lossless formats, exact byte equality after roundtrip.
