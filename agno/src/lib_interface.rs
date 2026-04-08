use std::{ffi::c_char, fs::File, os::raw::c_void, ptr::null_mut};

use tracing::{debug, info, warn};

#[cfg(feature = "webp")]
use crate::sony_jpeg::write_webp_from_rgb8_writer;
use crate::{
    agno_image::{AgnoImage, load::load_agno_image_from_file, scale_image},
    exif::ExifData,
    logging::{LogConfig, try_init},
};

/// Result of an FFI call that returns an AgnoImage.
/// On success: image is non-null, error is null.
/// On failure: image is null, error is a malloc'd UTF-8 string.
#[repr(C)]
pub struct AgnoResult {
    image: *mut AgnoImage,
    error: *mut u8,
    error_len: usize,
}

fn make_error_result(msg: String) -> AgnoResult {
    let bytes = msg.as_bytes();
    let ptr = unsafe { libc::malloc(bytes.len()) as *mut u8 };
    if ptr.is_null() {
        return AgnoResult {
            image: std::ptr::null_mut(),
            error: std::ptr::null_mut(),
            error_len: 0,
        };
    }
    unsafe {
        ptr.copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
    }
    AgnoResult {
        image: std::ptr::null_mut(),
        error: ptr,
        error_len: bytes.len(),
    }
}

macro_rules! into_result {
    ($expr:expr) => {
        match $expr {
            Ok(val) => AgnoResult {
                image: Box::into_raw(Box::new(val)),
                error: std::ptr::null_mut(),
                error_len: 0,
            },
            Err(e) => {
                let msg = format!("{e}");
                info!(error = msg, "Error occurred");
                make_error_result(msg)
            }
        }
    };
}

pub struct CString {
    data: *const u8,
    length: usize,
}

impl CString {
    // safety: data must point to memory allocated with malloc() that is valid
    // for `length` bytes. Returns Err on invalid UTF-8 — panicking across the
    // FFI boundary would be undefined behavior.
    pub fn new(data: *const u8, length: usize) -> Result<CString, &'static str> {
        unsafe {
            if std::str::from_utf8(std::slice::from_raw_parts(data, length)).is_err() {
                return Err("invalid utf-8 in path");
            }
        }
        Ok(CString { data, length })
    }

    pub fn as_str(&self) -> &str {
        unsafe {
            // from_utf8_unchecked is sound because we checked in the constructor
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.data, self.length))
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn load_image_from_path(path: *const c_char, len: usize) -> AgnoResult {
    let wrapped_path = match CString::new(path as *const u8, len) {
        Ok(p) => p,
        Err(e) => return make_error_result(e.to_string()),
    };
    into_result!(load_agno_image_from_file(wrapped_path.as_str()))
}

#[cfg(feature = "webp")]
#[unsafe(no_mangle)]
pub extern "C" fn write_agno_image_to_webp(path: *const c_char, len: usize, img: &mut AgnoImage) {
    let wrapped_path = match CString::new(path as *const u8, len) {
        Ok(p) => p,
        Err(e) => {
            warn!(
                error = e,
                "Invalid UTF-8 path passed to write_agno_image_to_webp"
            );
            return;
        }
    };
    let file = match File::create(wrapped_path.as_str()) {
        Ok(f) => f,
        Err(e) => {
            warn!(error = ?e, "Failed to create WebP file");
            return;
        }
    };

    if let Err(e) = write_webp_from_rgb8_writer(
        &mut std::io::BufWriter::new(file),
        img.as_slice(),
        img.width as u32,
        img.height as u32,
        90,
    ) {
        warn!(error = ?e, "Failed to encode WebP");
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn resize_image(
    img: *mut AgnoImage,
    new_width: usize,
    new_height: usize,
) -> AgnoResult {
    if img.is_null() {
        return make_error_result("resize_image called with null pointer".to_string());
    }

    unsafe {
        let real_img = Box::from_raw(img);
        into_result!(scale_image(*real_img, new_width as u32, new_height as u32))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn get_exif_value(img: &AgnoImage, img_tag: u16) -> ExifData {
    let data = img.exif.get_tag_value_by_tag(img_tag);

    let ret = match data {
        Some(value) => ExifData::from_exif_value(value),
        None => {
            debug!(
                "Tag {img_tag} not found in EXIF data ({} tags present)",
                img.exif.tag_count()
            );

            ExifData {
                data: null_mut(),
                len: 0,
                typ: 0,
            }
        }
    };

    if ret.len == 0 {
        return ret;
    }

    ret
}

#[repr(C)]
pub struct GpsCoordinates {
    pub lat: f64,
    pub lon: f64,
    pub valid: u8, // 1 = valid coords, 0 = no GPS data
}

#[unsafe(no_mangle)]
pub extern "C" fn get_gps_coordinates(img: &AgnoImage) -> GpsCoordinates {
    match img.exif.get_gps_coordinates() {
        Some((lat, lon)) => GpsCoordinates { lat, lon, valid: 1 },
        None => GpsCoordinates {
            lat: 0.0,
            lon: 0.0,
            valid: 0,
        },
    }
}

#[repr(C)]
pub struct AgnoBuffer {
    data: *mut u8,
    len: usize,
}

#[cfg(feature = "jpeg")]
#[unsafe(no_mangle)]
pub extern "C" fn write_agno_image_to_jpeg_buffer(img: &AgnoImage, quality: u8) -> AgnoBuffer {
    match img.to_jpeg(quality) {
        Ok(mut buf) => {
            buf.shrink_to_fit();
            let data = buf.as_mut_ptr();
            let len = buf.len();
            std::mem::forget(buf);
            AgnoBuffer { data, len }
        }
        Err(e) => {
            warn!(error = ?e, "Failed to encode JPEG");
            AgnoBuffer {
                data: null_mut(),
                len: 0,
            }
        }
    }
}

/// Load a specific page from a PDF file and return it as an AgnoImage.
/// `page_num` is 0-based. `max_width`/`max_height` of 0 uses default 2x scale.
#[cfg(feature = "pdf")]
#[unsafe(no_mangle)]
pub extern "C" fn load_pdf_page(
    path: *const c_char,
    len: usize,
    page_num: usize,
    max_width: u32,
    max_height: u32,
) -> AgnoResult {
    let wrapped_path = match CString::new(path as *const u8, len) {
        Ok(p) => p,
        Err(e) => return make_error_result(e.to_string()),
    };
    let max_dims = if max_width > 0 && max_height > 0 {
        Some((max_width, max_height))
    } else {
        None
    };

    let data: Result<Vec<u8>, Box<dyn std::error::Error>> =
        std::fs::read(wrapped_path.as_str()).map_err(|e| e.into());

    into_result!(data.and_then(|d| {
        crate::agno_image::load::load_pdf_page_from_bytes(
            &d,
            page_num,
            max_dims,
            crate::exif::ExifContext::default(),
        )
    }))
}

#[unsafe(no_mangle)]
pub extern "C" fn free_agno_buffer(buf: AgnoBuffer) {
    if !buf.data.is_null() && buf.len > 0 {
        unsafe {
            drop(Vec::from_raw_parts(buf.data, buf.len, buf.len));
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn free_agno_image(img: &AgnoImage) {
    AgnoImage::free(img);
}

#[unsafe(no_mangle)]
pub extern "C" fn free_agno_result(result: AgnoResult) {
    if !result.error.is_null() {
        unsafe {
            libc::free(result.error as *mut c_void);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn init_agno() {
    // Only initialize the logger once to avoid errors.
    let _ = try_init(LogConfig::library());

    info!("Agno initialized");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn into_result_returns_error_string_on_failure() {
        let result: Result<AgnoImage, Box<dyn std::error::Error>> =
            Err("decode failed: invalid JPEG header".into());

        let r = into_result!(result);
        assert!(r.image.is_null());
        assert!(!r.error.is_null());

        let msg = unsafe { std::slice::from_raw_parts(r.error, r.error_len) };
        assert_eq!(msg, b"decode failed: invalid JPEG header");

        free_agno_result(r);
    }

    #[test]
    fn into_result_returns_image_on_success() {
        let img = AgnoImage::new(vec![0u8; 3], 1, 1, crate::exif::ExifContext::default());
        let result: Result<AgnoImage, Box<dyn std::error::Error>> = Ok(img);

        let r = into_result!(result);
        assert!(!r.image.is_null());
        assert!(r.error.is_null());
        assert_eq!(r.error_len, 0);

        // Clean up
        unsafe {
            let _ = Box::from_raw(r.image);
        }
    }
}
