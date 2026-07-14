//! FFI lifecycle tests: drive the C ABI exactly as a C/Go caller would and
//! assert that load → resize → free cycles do not grow the Rust heap.
#![cfg(feature = "jpeg")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::CString;
use std::sync::atomic::{AtomicIsize, Ordering};

use agno::lib_interface::{free_agno_image, free_agno_result, load_image_from_path, resize_image};

struct CountingAllocator;

static LIVE_BYTES: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE_BYTES.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            LIVE_BYTES.fetch_add(
                new_size as isize - layout.size() as isize,
                Ordering::Relaxed,
            );
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The three tests below share the global `LIVE_BYTES` counter, so they must
/// not run concurrently or their heap-growth assertions will flake.
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

const TEST_IMAGE: &str = "../tests/data/sideways.jpeg";
const ORIENTATION_TAG: u16 = 0x0112;

fn c_path() -> (CString, usize) {
    let len = TEST_IMAGE.len();
    (CString::new(TEST_IMAGE).unwrap(), len)
}

/// One full caller cycle: load, resize to half size, free everything.
fn load_resize_free_cycle() {
    let (path, len) = c_path();

    let loaded = load_image_from_path(path.as_ptr(), len);
    assert!(loaded.error.is_null(), "load failed");
    assert!(!loaded.image.is_null());

    let (w, h) = unsafe { ((*loaded.image).width, (*loaded.image).height) };
    // resize_image consumes loaded.image (Box::from_raw on the Rust side)
    let resized = resize_image(loaded.image, (w / 2) as usize, (h / 2) as usize);
    free_agno_result(loaded);

    assert!(resized.error.is_null(), "resize failed");
    assert!(!resized.image.is_null());

    free_agno_image(resized.image);
    free_agno_result(resized);
}

/// One load-only cycle: load, free. Exercises the free path for originals.
fn load_free_cycle() {
    let (path, len) = c_path();

    let loaded = load_image_from_path(path.as_ptr(), len);
    assert!(loaded.error.is_null(), "load failed");
    assert!(!loaded.image.is_null());

    free_agno_image(loaded.image);
    free_agno_result(loaded);
}

#[test]
fn ffi_load_resize_free_cycles_do_not_grow_rust_heap() {
    let _guard = TEST_LOCK.lock().unwrap();

    // Warm up one-time lazy state (codec tables, GPU pipelines, logger).
    for _ in 0..3 {
        load_resize_free_cycle();
        load_free_cycle();
    }

    let before = LIVE_BYTES.load(Ordering::Relaxed);
    for _ in 0..32 {
        load_resize_free_cycle();
        load_free_cycle();
    }
    let growth = LIVE_BYTES.load(Ordering::Relaxed) - before;

    // Steady-state cycles must not accumulate Rust-heap memory. The EXIF
    // table of one image is tens of KB across 64 frees; allow generous
    // slack for allocator jitter while staying far below one leaked cycle.
    assert!(
        growth < 4096,
        "Rust heap grew by {growth} bytes over 64 load/free cycles — \
         freed images are leaking their ExifContext"
    );
}

#[test]
fn resize_preserves_exif() {
    use agno::lib_interface::{free_exif_data, get_exif_value};

    let _guard = TEST_LOCK.lock().unwrap();

    let (path, len) = c_path();
    let loaded = load_image_from_path(path.as_ptr(), len);
    assert!(loaded.error.is_null());

    let (w, h) = unsafe { ((*loaded.image).width, (*loaded.image).height) };
    let resized = resize_image(loaded.image, (w / 2) as usize, (h / 2) as usize);
    free_agno_result(loaded);
    assert!(resized.error.is_null());

    // Orientation was applied at load (auto-rotate sets it to 1) but the
    // tag must still exist on the resized image.
    let exif = unsafe { get_exif_value(&*resized.image, ORIENTATION_TAG) };
    assert!(
        !exif.data.is_null() && exif.len > 0,
        "resized image lost its EXIF data"
    );
    free_exif_data(exif);

    free_agno_image(resized.image);
    free_agno_result(resized);
}

#[test]
fn exif_value_reads_do_not_grow_rust_heap_and_buffers_are_freeable() {
    use agno::lib_interface::{free_exif_data, get_exif_value};

    let _guard = TEST_LOCK.lock().unwrap();

    let (path, len) = c_path();
    let loaded = load_image_from_path(path.as_ptr(), len);
    assert!(loaded.error.is_null());
    let img = loaded.image;

    // Warm up.
    for _ in 0..3 {
        let v = unsafe { get_exif_value(&*img, ORIENTATION_TAG) };
        free_exif_data(v);
    }

    let before = LIVE_BYTES.load(Ordering::Relaxed);
    for _ in 0..64 {
        let v = unsafe { get_exif_value(&*img, ORIENTATION_TAG) };
        assert!(!v.data.is_null());
        free_exif_data(v);
    }
    let growth = LIVE_BYTES.load(Ordering::Relaxed) - before;
    assert!(
        growth < 1024,
        "Rust heap grew by {growth} bytes over 64 get_exif_value calls"
    );

    free_agno_image(img);
    free_agno_result(loaded);
}
