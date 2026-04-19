/// Integration tests for image resize (GPU and CPU paths).
use agno::codec::jpeg::decode_jpeg;

/// Returns true when the test should be skipped because it requires GPU.
fn skip_if_no_gpu() -> bool {
    #[cfg(feature = "gpu")]
    {
        agno::gpu::GpuContext::get().is_none()
    }
    #[cfg(not(feature = "gpu"))]
    {
        true
    }
}

fn load_test_image() -> (Vec<u8>, u32, u32) {
    let data = std::fs::read("../tests/data/sideways.jpeg").expect("test image");
    let (rgb, w, h) = decode_jpeg(&data).expect("decode");
    (rgb, w, h)
}

#[test]
fn gpu_resize_produces_correct_dimensions() {
    let (rgb, w, h) = load_test_image();
    let target_w = w / 4;
    let target_h = h / 4;

    #[cfg(feature = "gpu")]
    {
        if let Some(result) = agno::resize_gpu::resize_gpu(&rgb, w, h, target_w, target_h) {
            assert_eq!(
                result.len(),
                (target_w * target_h * 3) as usize,
                "GPU resize output size wrong"
            );
            eprintln!(
                "GPU resize: {}x{} → {}x{} ({} bytes)",
                w,
                h,
                target_w,
                target_h,
                result.len()
            );
        } else {
            eprintln!("GPU unavailable — skipping GPU resize assertion");
        }
    }
}

#[test]
fn cpu_resize_produces_correct_dimensions() {
    let (rgb, w, h) = load_test_image();
    let target_w = w / 4;
    let target_h = h / 4;

    let result = agno::agno_image::ops::resize_lanczos3(
        &rgb,
        w as usize,
        h as usize,
        target_w as usize,
        target_h as usize,
        3,
    );
    assert_eq!(result.len(), (target_w * target_h * 3) as usize);
}

/// Verify that resize_gpu returns None (not panics) when any storage buffer
/// would exceed the device's max_storage_buffer_binding_size.
///
/// On Metal/macOS this limit is typically 128 MB; an ~48 MP+ image packed
/// as u32 would exceed it. Pre-fix: wgpu triggered a fatal validation error
/// (SIGABRT via CGo). Post-fix: the function returns None before creating
/// any buffer, allowing the CPU fallback to handle the image.
///
/// We pass an empty RGB slice because the guard checks w×h×4 arithmetic and
/// returns None before accessing the slice — the test is valid and allocates
/// no image memory.
#[test]
fn gpu_resize_returns_none_for_oversized_input() {
    if skip_if_no_gpu() {
        eprintln!("No GPU available, skipping gpu_resize_returns_none_for_oversized_input");
        return;
    }
    #[cfg(feature = "gpu")]
    {
        let max_binding = agno::gpu::GpuContext::get()
            .unwrap()
            .device
            .limits()
            .max_storage_buffer_binding_size as u64;

        // One pixel over the limit (packed as u32 = 4 bytes/pixel)
        let w = (max_binding / 4 + 1) as u32;
        let h = 1u32;

        // Empty slice: the size guard fires before the slice is read.
        let result = agno::resize_gpu::resize_gpu(&[], w, h, 1, 1);
        assert!(
            result.is_none(),
            "Expected None for oversized input ({} bytes > {} limit), got Some",
            w as u64 * 4,
            max_binding
        );
        eprintln!(
            "gpu_resize_returns_none_for_oversized_input: correctly returned None for {} MB input (limit {} MB)",
            w as u64 * 4 / 1024 / 1024,
            max_binding / 1024 / 1024
        );
    }
}

#[test]
fn resize_timing_comparison() {
    let (rgb, w, h) = load_test_image();
    let target_w = 256u32;
    let target_h = 256u32;
    let iterations: u128 = 5;

    // CPU timing
    let cpu_start = std::time::Instant::now();
    for _ in 0..iterations {
        let _ = agno::agno_image::ops::resize_lanczos3(
            &rgb,
            w as usize,
            h as usize,
            target_w as usize,
            target_h as usize,
            3,
        );
    }
    let cpu_ms = cpu_start.elapsed().as_millis() / iterations;
    eprintln!(
        "CPU Lanczos3: ~{}ms per resize ({}x{} → {}x{})",
        cpu_ms, w, h, target_w, target_h
    );

    // GPU timing
    #[cfg(feature = "gpu")]
    {
        let gpu_start = std::time::Instant::now();
        let mut gpu_succeeded = false;
        for _ in 0..iterations {
            if agno::resize_gpu::resize_gpu(&rgb, w, h, target_w, target_h).is_some() {
                gpu_succeeded = true;
            }
        }
        if gpu_succeeded {
            let gpu_ms = gpu_start.elapsed().as_millis() / iterations;
            eprintln!(
                "GPU Lanczos3: ~{}ms per resize ({}x{} → {}x{})",
                gpu_ms, w, h, target_w, target_h
            );
        } else {
            eprintln!("GPU unavailable — no GPU timing");
        }
    }
}
