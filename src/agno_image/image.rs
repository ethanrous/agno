use std::{os::raw::c_void, ptr::null_mut};

use libc::c_uchar;

use crate::exif::ExifContext;

#[repr(C)] // Ensure C-compatible layout
pub struct AgnoImage {
    data: *mut c_uchar,
    len: usize,

    pub width: u64,
    pub height: u64,

    pub exif: ExifContext,
}

impl AgnoImage {
    pub fn new(data: Vec<u8>, width: u64, height: u64, exif_ctx: ExifContext) -> Self {
        let pixels_rgb = unsafe { libc::malloc((width * height * 3) as usize) as *mut c_uchar };
        if pixels_rgb.is_null() {
            return AgnoImage {
                data: null_mut(),
                exif: exif_ctx,
                len: 0,
                height: 0,
                width: 0,
            };
        }

        unsafe {
            pixels_rgb.copy_from_nonoverlapping(data.as_ptr(), data.len());
        }

        AgnoImage {
            data: pixels_rgb,
            exif: exif_ctx,
            len: data.len(),
            height,
            width,
        }
    }

    pub fn null() -> *mut AgnoImage {
        null_mut()
    }

    pub fn free(img: &AgnoImage) {
        unsafe {
            if !img.data.is_null() {
                libc::free(img.data as *mut c_void);
            }

            // img is dropped here (struct memory freed)
            // Box::from_raw(&mut img);
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data, self.len) }
    }
}
