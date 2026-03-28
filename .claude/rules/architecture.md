# Architecture

## Crate Dependency Graph

```
agno (main crate)
├── agno-gpu-shared (shared CPU/GPU types, no_std compatible)
└── [build-dep] spirv-builder → compiles agno-gpu-kernels → SPIR-V binary
```

At build time, `agno/build.rs` invokes `spirv-builder` to compile `agno-gpu-kernels` into a SPIR-V binary. The binary is embedded into `agno` via `include_bytes!()`. At runtime, wgpu loads it.

## Module Map

```
agno/src/
├── lib.rs                  Module declarations, feature-gated
├── main.rs                 CLI (exif, convert, resize subcommands)
├── lib_interface.rs        C FFI exports (#[no_mangle] extern "C")
├── logging.rs              tracing-subscriber init
│
├── agno_image/             Core image types and pipeline
│   ├── image.rs            AgnoImage — #[repr(C)] image struct (RGB8, libc::malloc'd)
│   ├── ops.rs              Pixel operations (resize, rotate, flip)
│   ├── transform.rs        High-level transforms (scale_image, auto_rotate_image)
│   └── load/               Format-specific loaders
│       ├── load.rs          Format auto-detection by magic bytes, dispatch to loader
│       ├── sony.rs          Sony ARW RAW (thin wrapper, calls sony_decoder.rs)
│       ├── canon.rs         Canon CR2 RAW (thin wrapper, calls canon_decoder.rs)
│       ├── heic.rs          HEIC/HEIF (parses container, decodes HEVC)
│       ├── mov.rs           MOV/MP4 thumbnail extraction (4 strategies)
│       └── pdf.rs           PDF (optional, incomplete)
│
├── codec/                  Native format implementations (no external C deps)
│   ├── heif.rs             HEIF container parser (single-tile and grid)
│   ├── isobmff.rs          ISO Base Media Format box navigation
│   ├── hevc/               Full HEVC/H.265 still-image decoder
│   ├── jpeg/               JPEG encoder + decoder (DCT, Huffman, quantization)
│   ├── webp/               WebP encoder + decoder (VP8, arithmetic coding)
│   └── png/                PNG decoder
│
├── exif/                   EXIF metadata
│   ├── mod.rs              ExifContext (HashMap<u16, ExifValue>), parse/cache/query
│   └── spec.rs             EXIF field definitions (ExifField struct, ExifSection enum, tag catalog)
│
├── gpu/                    GPU infrastructure (#[cfg(feature = "gpu")])
│   ├── context.rs          GpuContext singleton (OnceLock<Option<GpuContext>>)
│   └── pipeline.rs         Compute dispatch helpers (buffers, bind groups, readback)
│
├── demosaic.rs             CPU Bayer demosaicing
├── demosaic_gpu.rs         GPU Bayer demosaicing
├── resize_gpu.rs           GPU Lanczos3 resize
├── jpeg_gpu.rs             GPU JPEG DCT acceleration
├── webp_gpu.rs             GPU WebP encoding
├── sony_decoder.rs         Sony RAW decompression (called by load/sony.rs)
├── sony_jpeg.rs            Sony embedded JPEG utilities
├── canon_decoder.rs        Canon RAW LJPEG decompression (called by load/canon.rs)
└── tiff.rs                 TIFF/IFD parser (used by RAW + EXIF)
```

## Image Pipeline

### Load path (file → AgnoImage)

```
File → detect_image_type() [magic bytes]
     → route to format loader:
        Standard: codec decode → RGB8 Vec<u8>
        RAW:      TIFF parse → extract sensor data → demosaic → color correct → RGB8
        HEIC:     HEIF container parse → HEVC decode → YUV→RGB → RGB8
        MOV:      try 4 strategies (HEIF items, thumbnail track, UDTA JPEG, HEVC keyframe)
     → AgnoImage::new(rgb_data, width, height, exif)
        [copies data into libc::malloc'd buffer for FFI safety]
```

### Transform path

```
AgnoImage → scale_image(width, height)
              [GPU resize_gpu → Option<Vec<u8>>, fallback to CPU Lanczos3]
          → auto_rotate_image(exif orientation 1-8)
          → new AgnoImage [old one freed]
```

### Encode path

```
AgnoImage → .to_jpeg(quality) → Vec<u8>     (native JPEG encoder)
          → .to_webp(quality) → Vec<u8>     (native WebP encoder)
          → write to file
```

## Integration with Weblens

Agno is a git submodule of the Weblens project. The parent builds it via `just build` (or directly via `build/sh/build-agno.bash`), producing `libagno.a`. The Go backend links this via CGO. The FFI boundary is defined in `lib_interface.rs`; C declarations are consumed directly by Go's CGO bindings in the parent project (no separate `.h` header file in this repo).
