use std::{fs::File, ptr::null_mut};

use log::{LevelFilter, info};

use crate::{
    agno_image::{AgnoImage, load::load_agno_image_from_file, scale_image},
    exif::ExifData,
    sony_jpeg::write_webp_from_rgb8_writer,
};

macro_rules! ok_or_null {
    ($expr:expr) => {
        match $expr {
            Ok(val) => Box::into_raw(Box::new(val)),
            Err(e) => {
                info!("Error occurred, returning null pointer: {:?}", e);
                AgnoImage::null()
            }
        }
    };
}

pub struct CString {
    data: *const u8,
    length: usize,
}

impl CString {
    // safety: data must point to nul-terminated memory allocated with malloc()
    pub fn new(data: *const u8, length: usize) -> CString {
        // Note: no reallocation happens here, we use `str::from_utf8()` only to
        // check whether the pointer contains valid UTF-8.
        // If panic is unacceptable, the constructor can return a `Result`
        unsafe {
            if std::str::from_utf8(std::slice::from_raw_parts(data, length)).is_err() {
                panic!("invalid utf-8")
            }
        }
        CString { data, length }
    }

    pub fn as_str(&self) -> &str {
        unsafe {
            // from_utf8_unchecked is sound because we checked in the constructor
            std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.data, self.length))
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn load_image_from_path(path: *const u8, len: usize) -> *mut AgnoImage {
    let wrapped_path = CString::new(path, len);

    ok_or_null!(load_agno_image_from_file(wrapped_path.as_str()))
}

#[unsafe(no_mangle)]
pub extern "C" fn write_agno_image_to_webp(path: *const u8, len: usize, img: &mut AgnoImage) {
    let wrapped_path = CString::new(path, len);
    let mut file = File::create(wrapped_path.as_str()).unwrap();

    let _ = write_webp_from_rgb8_writer(
        &mut file,
        img.as_slice(),
        img.width as u32,
        img.height as u32,
        90,
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn resize_image(
    img: *mut AgnoImage,
    new_width: usize,
    new_height: usize,
) -> *mut AgnoImage {
    if img.is_null() {
        return AgnoImage::null();
    }

    unsafe {
        let real_img = Box::from_raw(img);
        let new_img = ok_or_null!(scale_image(*real_img, new_width as u32, new_height as u32));

        new_img
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn get_exif_value(img: &AgnoImage, img_tag: u16) -> ExifData {
    let data = img.exif.get_tag_value_by_tag(img_tag);

    let ret = match data {
        Some(value) => ExifData::from_exif_value(value),
        None => ExifData {
            data: null_mut(),
            len: 0,
            typ: 0,
        },
    };

    if ret.len == 0 {
        return ret;
    }

    ret
}

#[unsafe(no_mangle)]
pub extern "C" fn free_agno_image(img: &AgnoImage) {
    AgnoImage::free(img);
}

#[unsafe(no_mangle)]
pub extern "C" fn init_agno() {
    // Only initialize the logger once to avoid errors.
    let _ = env_logger::builder()
        .filter_level(LevelFilter::Debug) // Set default log level
        .try_init();

    info!("Agno initialized");
}
