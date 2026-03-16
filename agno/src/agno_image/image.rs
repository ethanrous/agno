use std::{error::Error, os::raw::c_void, ptr::null_mut};

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

    #[allow(dead_code)]
    pub fn null() -> *mut AgnoImage {
        null_mut()
    }

    #[allow(dead_code)]
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

    /// Apply EXIF orientation transform in-place, then no-op on future calls.
    pub fn auto_rotate(&mut self) -> Result<(), Box<dyn Error>> {
        use crate::exif::{ExifValue, spec::ORIENTATION};
        use crate::sony_decoder::Dimensions;

        let orientation = match self.exif.get_tag_value(ORIENTATION) {
            Some(ExifValue::Short(v)) if !v.is_empty() => v[0] as u8,
            _ => return Ok(()),
        };
        if orientation == 1 {
            return Ok(());
        }

        let mut dims = Dimensions {
            raw_width: self.width as usize,
            raw_height: self.height as usize,
            output_width: self.width as usize,
            output_height: self.height as usize,
        };
        let data = self.as_slice().to_vec();
        let rotated = super::auto_rotate_image(&mut self.exif, &data, &mut dims)?;

        unsafe {
            if !self.data.is_null() {
                libc::free(self.data as *mut c_void);
            }
            let new_ptr = libc::malloc(rotated.len()) as *mut c_uchar;
            new_ptr.copy_from_nonoverlapping(rotated.as_ptr(), rotated.len());
            self.data = new_ptr;
            self.len = rotated.len();
        }
        self.width = dims.output_width as u64;
        self.height = dims.output_height as u64;

        // Mark orientation as applied so no downstream consumer re-rotates
        self.exif.set_tag_value(ORIENTATION, ExifValue::Short(vec![1]));

        Ok(())
    }

    #[allow(dead_code)]
    pub fn to_jpeg(&self, quality: u8) -> Result<Vec<u8>, Box<dyn Error>> {
        let img = image::RgbImage::from_raw(
            self.width as u32,
            self.height as u32,
            self.as_slice().to_vec(),
        )
        .unwrap();
        let mut buf = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);

        encoder.encode_image(&img)?;

        Ok(buf)
    }

    #[allow(dead_code)]
    pub fn to_jpeg_file(&self, quality: u8, path: &str) -> Result<(), Box<dyn Error>> {
        let buf = self.to_jpeg(quality)?;

        std::fs::write(path, buf)?;

        Ok(())
    }
}
