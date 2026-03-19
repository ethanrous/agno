/// HEVC picture buffer with YCbCr 4:2:0 planes and color space conversion.
///
/// Stores decoded HEVC frame data as separate luma/chroma planes and provides
/// conversion to interleaved RGB8 using BT.709 or BT.601 color matrices.

/// ITU-T H.265 matrix coefficients identifier.
/// Determines which YCbCr-to-RGB conversion matrix to apply.
const MATRIX_COEFFS_BT709: u8 = 1;
const MATRIX_COEFFS_BT601_525: u8 = 5;
const MATRIX_COEFFS_BT601_625: u8 = 6;

/// SAO parameters for a single component of a single CTU.
#[derive(Clone, Default)]
pub struct SaoParams {
    /// 0 = not applied, 1 = band offset, 2 = edge offset
    pub sao_type_idx: u8,
    /// 4 offsets for band or edge offset categories
    pub sao_offset: [i8; 4],
    /// Starting band position (for band offset, type 1)
    pub sao_band_position: u8,
    /// Edge class: 0=horizontal, 1=vertical, 2=diag-135, 3=diag-45 (for edge offset, type 2)
    pub sao_eo_class: u8,
}

/// SAO parameters for all 3 components of a single CTU.
#[derive(Clone, Default)]
pub struct CtuSaoParams {
    pub y: SaoParams,
    pub cb: SaoParams,
    pub cr: SaoParams,
}

/// Decoded HEVC picture in YCbCr 4:2:0 planar format.
///
/// Luma plane is full resolution. Chroma planes (Cb, Cr) are half resolution
/// in both dimensions per the 4:2:0 subsampling scheme.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    y: Vec<i16>,
    cb: Vec<i16>,
    cr: Vec<i16>,
    pub stride_y: u32,
    pub stride_c: u32,
    pub bit_depth: u8,
    /// ITU-T H.265 matrix_coefficients: 1 = BT.709 (default), 5/6 = BT.601
    pub matrix_coeffs: u8,
    /// SAO parameters per CTU, indexed by CTU raster-scan address.
    /// Populated during slice decoding, applied as post-processing.
    pub sao_params: Vec<CtuSaoParams>,
    /// CU quadtree depth at MinCB granularity (0xFF = unavailable).
    cu_depth: Vec<u8>,
    /// Luma intra prediction mode at MinCB granularity (0..34).
    intra_mode: Vec<u8>,
    /// Grid width for cu_depth/intra_mode maps: ceil(pic_width / min_cb_size).
    depth_stride: u32,
    /// log2 of the minimum coding block size (needed by accessors).
    min_cb_log2: u32,
}

impl Picture {
    /// Allocate a new picture with zeroed planes.
    ///
    /// Chroma dimensions are `width/2` x `height/2` for 4:2:0 subsampling.
    /// Strides equal dimensions (no alignment padding).
    pub fn new(width: u32, height: u32, bit_depth: u8) -> Self {
        let chroma_w = (width + 1) / 2;
        let chroma_h = (height + 1) / 2;

        let luma_len = (width as usize) * (height as usize);
        let chroma_len = (chroma_w as usize) * (chroma_h as usize);

        Self {
            width,
            height,
            y: vec![0i16; luma_len],
            cb: vec![0i16; chroma_len],
            cr: vec![0i16; chroma_len],
            stride_y: width,
            stride_c: chroma_w,
            bit_depth,
            matrix_coeffs: MATRIX_COEFFS_BT709,
            sao_params: Vec::new(),
            cu_depth: Vec::new(),
            intra_mode: Vec::new(),
            depth_stride: 0,
            min_cb_log2: 0,
        }
    }

    /// Whether per-CU metadata maps have been initialized.
    /// Returns true after `init_metadata()` has been called.
    pub fn has_metadata(&self) -> bool {
        !self.cu_depth.is_empty()
    }

    /// Allocate per-CU metadata maps at MinCB granularity.
    pub fn init_metadata(&mut self, min_cb_log2: u32) {
        let min_cb = 1u32 << min_cb_log2;
        let w = (self.width + min_cb - 1) / min_cb;
        let h = (self.height + min_cb - 1) / min_cb;
        let len = (w * h) as usize;
        self.cu_depth = vec![0xFF; len];
        self.intra_mode = vec![0; len];
        self.depth_stride = w;
        self.min_cb_log2 = min_cb_log2;
    }

    /// Store CU depth for all MinCB cells covered by a CU at (x0, y0) with given size.
    pub fn set_cu_depth(&mut self, x0: u32, y0: u32, cu_size: u32, depth: u8) {
        if self.cu_depth.is_empty() { return; }
        let min_cb = 1u32 << self.min_cb_log2;
        let gx = x0 / min_cb;
        let gy = y0 / min_cb;
        let cells = cu_size / min_cb;
        for dy in 0..cells {
            for dx in 0..cells {
                let ix = gx + dx;
                let iy = gy + dy;
                if ix < self.depth_stride {
                    let idx = (iy * self.depth_stride + ix) as usize;
                    if idx < self.cu_depth.len() {
                        self.cu_depth[idx] = depth;
                    }
                }
            }
        }
    }

    /// Read CU depth at sample position (x, y). Returns None if unavailable.
    pub fn cu_depth_at(&self, x: u32, y: u32) -> Option<u8> {
        if self.cu_depth.is_empty() { return None; }
        let min_cb = 1u32 << self.min_cb_log2;
        let gx = x / min_cb;
        let gy = y / min_cb;
        let idx = (gy * self.depth_stride + gx) as usize;
        self.cu_depth.get(idx).copied().filter(|&d| d != 0xFF)
    }

    /// Store intra prediction mode for all MinCB cells covered by a PU.
    pub fn set_intra_mode(&mut self, x0: u32, y0: u32, pu_size: u32, mode: u8) {
        if self.intra_mode.is_empty() { return; }
        let min_cb = 1u32 << self.min_cb_log2;
        let gx = x0 / min_cb;
        let gy = y0 / min_cb;
        let cells = (pu_size + min_cb - 1) / min_cb;
        for dy in 0..cells {
            for dx in 0..cells {
                let ix = gx + dx;
                let iy = gy + dy;
                if ix < self.depth_stride {
                    let idx = (iy * self.depth_stride + ix) as usize;
                    if idx < self.intra_mode.len() {
                        self.intra_mode[idx] = mode;
                    }
                }
            }
        }
    }

    /// Read intra mode at sample position. Returns 0 (Planar) if unavailable.
    pub fn intra_mode_at(&self, x: u32, y: u32) -> u8 {
        if self.intra_mode.is_empty() { return 0; }
        let min_cb = 1u32 << self.min_cb_log2;
        let gx = x / min_cb;
        let gy = y / min_cb;
        let idx = (gy * self.depth_stride + gx) as usize;
        self.intra_mode.get(idx).copied().unwrap_or(0)
    }

    /// Immutable slice of the full luma plane.
    pub fn y_plane(&self) -> &[i16] {
        &self.y
    }

    /// Immutable slice of the Cb (chroma blue) plane.
    pub fn cb_plane(&self) -> &[i16] {
        &self.cb
    }

    /// Immutable slice of the Cr (chroma red) plane.
    pub fn cr_plane(&self) -> &[i16] {
        &self.cr
    }

    /// Mutable slice of the full luma plane.
    pub fn y_mut(&mut self) -> &mut [i16] {
        &mut self.y
    }

    /// Mutable slice of the Cb (chroma blue) plane.
    pub fn cb_mut(&mut self) -> &mut [i16] {
        &mut self.cb
    }

    /// Mutable slice of the Cr (chroma red) plane.
    pub fn cr_mut(&mut self) -> &mut [i16] {
        &mut self.cr
    }

    /// Read luma sample at pixel coordinates (x, y).
    pub fn y_at(&self, x: u32, y: u32) -> i16 {
        self.y[(y * self.stride_y + x) as usize]
    }

    /// Read Cb sample at chroma coordinates (x, y).
    pub fn cb_at(&self, x: u32, y: u32) -> i16 {
        self.cb[(y * self.stride_c + x) as usize]
    }

    /// Read Cr sample at chroma coordinates (x, y).
    pub fn cr_at(&self, x: u32, y: u32) -> i16 {
        self.cr[(y * self.stride_c + x) as usize]
    }

    /// Write luma sample at pixel coordinates (x, y).
    pub fn set_y(&mut self, x: u32, y: u32, val: i16) {
        let idx = (y * self.stride_y + x) as usize;
        self.y[idx] = val;
    }

    /// Write Cb sample at chroma coordinates (x, y).
    pub fn set_cb(&mut self, x: u32, y: u32, val: i16) {
        let idx = (y * self.stride_c + x) as usize;
        self.cb[idx] = val;
    }

    /// Write Cr sample at chroma coordinates (x, y).
    pub fn set_cr(&mut self, x: u32, y: u32, val: i16) {
        let idx = (y * self.stride_c + x) as usize;
        self.cr[idx] = val;
    }

    /// Convert the YCbCr 4:2:0 picture to interleaved RGB8.
    ///
    /// Chroma is upsampled from half-resolution using nearest-neighbor.
    /// Color matrix selection is based on `matrix_coeffs`:
    /// - 1 (default): BT.709
    /// - 5, 6: BT.601
    ///
    /// For 10-bit content, samples are right-shifted by 2 before conversion.
    pub fn to_rgb8(&self) -> Vec<u8> {
        let w = self.width as usize;
        let h = self.height as usize;
        let stride_y = self.stride_y as usize;
        let stride_c = self.stride_c as usize;
        let shift = if self.bit_depth > 8 {
            (self.bit_depth - 8) as i16
        } else {
            0
        };

        let coeffs = matrix_for(self.matrix_coeffs);
        let mut rgb = vec![0u8; w * h * 3];

        for py in 0..h {
            let cy = py / 2;
            let chroma_h_max = ((self.height as usize + 1) / 2).saturating_sub(1);
            let cy_clamped = cy.min(chroma_h_max);
            let row_y_base = py * stride_y;
            let row_c_base = cy_clamped * stride_c;
            let rgb_row_base = py * w * 3;

            for px in 0..w {
                let cx = px / 2;
                let chroma_w_max = ((self.width as usize + 1) / 2).saturating_sub(1);
                let cx_clamped = cx.min(chroma_w_max);

                let y_val = self.y[row_y_base + px] >> shift;
                let cb_val = self.cb[row_c_base + cx_clamped] >> shift;
                let cr_val = self.cr[row_c_base + cx_clamped] >> shift;

                let (r, g, b) = ycbcr_to_rgb(y_val, cb_val, cr_val, &coeffs);

                let out = rgb_row_base + px * 3;
                rgb[out] = r;
                rgb[out + 1] = g;
                rgb[out + 2] = b;
            }
        }

        rgb
    }
}

/// BT.709 / BT.601 conversion coefficients.
///
/// YCbCr to RGB:
///   R = Y + cr_r * (Cr - 128)
///   G = Y + cb_g * (Cb - 128) + cr_g * (Cr - 128)
///   B = Y + cb_b * (Cb - 128)
struct ColorCoeffs {
    cr_r: f64,
    cb_g: f64,
    cr_g: f64,
    cb_b: f64,
}

const BT709: ColorCoeffs = ColorCoeffs {
    cr_r: 1.5748,
    cb_g: -0.1873,
    cr_g: -0.4681,
    cb_b: 1.8556,
};

const BT601: ColorCoeffs = ColorCoeffs {
    cr_r: 1.402,
    cb_g: -0.344,
    cr_g: -0.714,
    cb_b: 1.772,
};

fn matrix_for(matrix_coeffs: u8) -> &'static ColorCoeffs {
    match matrix_coeffs {
        MATRIX_COEFFS_BT601_525 | MATRIX_COEFFS_BT601_625 => &BT601,
        _ => &BT709,
    }
}

/// Convert a single YCbCr sample to RGB using the given color matrix.
/// Cb and Cr are expected centered at 128 (8-bit range).
fn ycbcr_to_rgb(y: i16, cb: i16, cr: i16, c: &ColorCoeffs) -> (u8, u8, u8) {
    let yf = y as f64;
    let cb_off = (cb - 128) as f64;
    let cr_off = (cr - 128) as f64;

    let r = yf + c.cr_r * cr_off;
    let g = yf + c.cb_g * cb_off + c.cr_g * cr_off;
    let b = yf + c.cb_b * cb_off;

    (clip_u8(r), clip_u8(g), clip_u8(b))
}

fn clip_u8(v: f64) -> u8 {
    if v < 0.0 {
        0
    } else if v > 255.0 {
        255
    } else {
        (v + 0.5) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_allocates_correct_sizes() {
        let pic = Picture::new(1920, 1080, 8);
        assert_eq!(pic.y.len(), 1920 * 1080);
        assert_eq!(pic.cb.len(), 960 * 540);
        assert_eq!(pic.cr.len(), 960 * 540);
        assert_eq!(pic.stride_y, 1920);
        assert_eq!(pic.stride_c, 960);
        assert_eq!(pic.bit_depth, 8);
        assert_eq!(pic.matrix_coeffs, MATRIX_COEFFS_BT709);
    }

    #[test]
    fn new_odd_dimensions() {
        let pic = Picture::new(7, 5, 10);
        // chroma: (7+1)/2 = 4, (5+1)/2 = 3
        assert_eq!(pic.cb.len(), 4 * 3);
        assert_eq!(pic.stride_c, 4);
    }

    #[test]
    fn set_and_get_samples() {
        let mut pic = Picture::new(4, 4, 8);
        pic.set_y(2, 3, 200);
        assert_eq!(pic.y_at(2, 3), 200);

        pic.set_cb(1, 1, -50);
        assert_eq!(pic.cb_at(1, 1), -50);

        pic.set_cr(0, 0, 300);
        assert_eq!(pic.cr_at(0, 0), 300);
    }

    #[test]
    fn mutable_plane_access() {
        let mut pic = Picture::new(2, 2, 8);
        let y = pic.y_mut();
        y[0] = 100;
        y[1] = 110;
        y[2] = 120;
        y[3] = 130;
        assert_eq!(pic.y_at(0, 0), 100);
        assert_eq!(pic.y_at(1, 0), 110);
        assert_eq!(pic.y_at(0, 1), 120);
        assert_eq!(pic.y_at(1, 1), 130);
    }

    #[test]
    fn neutral_gray_bt709() {
        // Y=128, Cb=128, Cr=128 should produce gray (128, 128, 128)
        let mut pic = Picture::new(2, 2, 8);
        for s in pic.y_mut().iter_mut() {
            *s = 128;
        }
        for s in pic.cb_mut().iter_mut() {
            *s = 128;
        }
        for s in pic.cr_mut().iter_mut() {
            *s = 128;
        }

        let rgb = pic.to_rgb8();
        assert_eq!(rgb.len(), 2 * 2 * 3);
        for chunk in rgb.chunks(3) {
            assert_eq!(chunk[0], 128);
            assert_eq!(chunk[1], 128);
            assert_eq!(chunk[2], 128);
        }
    }

    #[test]
    fn pure_white_bt709() {
        let mut pic = Picture::new(2, 2, 8);
        for s in pic.y_mut().iter_mut() {
            *s = 255;
        }
        for s in pic.cb_mut().iter_mut() {
            *s = 128;
        }
        for s in pic.cr_mut().iter_mut() {
            *s = 128;
        }

        let rgb = pic.to_rgb8();
        for chunk in rgb.chunks(3) {
            assert_eq!(chunk[0], 255);
            assert_eq!(chunk[1], 255);
            assert_eq!(chunk[2], 255);
        }
    }

    #[test]
    fn pure_black() {
        let mut pic = Picture::new(2, 2, 8);
        // Y=0, Cb=128, Cr=128
        for s in pic.cb_mut().iter_mut() {
            *s = 128;
        }
        for s in pic.cr_mut().iter_mut() {
            *s = 128;
        }

        let rgb = pic.to_rgb8();
        for chunk in rgb.chunks(3) {
            assert_eq!(chunk[0], 0);
            assert_eq!(chunk[1], 0);
            assert_eq!(chunk[2], 0);
        }
    }

    #[test]
    fn clipping_prevents_overflow() {
        let mut pic = Picture::new(2, 2, 8);
        for s in pic.y_mut().iter_mut() {
            *s = 255;
        }
        // Max Cr drives R channel high, should clip at 255
        for s in pic.cr_mut().iter_mut() {
            *s = 255;
        }
        for s in pic.cb_mut().iter_mut() {
            *s = 128;
        }

        let rgb = pic.to_rgb8();
        assert_eq!(rgb[0], 255); // R clipped
    }

    #[test]
    fn clipping_prevents_underflow() {
        let mut pic = Picture::new(2, 2, 8);
        // Y=0 with extreme chroma should clip at 0
        for s in pic.cr_mut().iter_mut() {
            *s = 255;
        }
        for s in pic.cb_mut().iter_mut() {
            *s = 128;
        }

        let rgb = pic.to_rgb8();
        // G channel: 0 + (-0.1873)*0 + (-0.4681)*(255-128) = -59.4 -> clips to 0
        assert_eq!(rgb[1], 0);
    }

    #[test]
    fn ten_bit_shift() {
        // 10-bit value 512 >> 2 = 128, which with neutral chroma gives gray
        let mut pic = Picture::new(2, 2, 10);
        for s in pic.y_mut().iter_mut() {
            *s = 512;
        }
        for s in pic.cb_mut().iter_mut() {
            *s = 512;
        }
        for s in pic.cr_mut().iter_mut() {
            *s = 512;
        }

        let rgb = pic.to_rgb8();
        for chunk in rgb.chunks(3) {
            assert_eq!(chunk[0], 128);
            assert_eq!(chunk[1], 128);
            assert_eq!(chunk[2], 128);
        }
    }

    #[test]
    fn bt601_matrix_selection() {
        let mut pic = Picture::new(2, 2, 8);
        pic.matrix_coeffs = MATRIX_COEFFS_BT601_525;
        for s in pic.y_mut().iter_mut() {
            *s = 128;
        }
        for s in pic.cb_mut().iter_mut() {
            *s = 128;
        }
        for s in pic.cr_mut().iter_mut() {
            *s = 128;
        }

        // Neutral chroma produces identical result regardless of matrix
        let rgb = pic.to_rgb8();
        for chunk in rgb.chunks(3) {
            assert_eq!(chunk[0], 128);
            assert_eq!(chunk[1], 128);
            assert_eq!(chunk[2], 128);
        }
    }

    #[test]
    fn bt601_red_differs_from_bt709() {
        // With non-neutral chroma, BT.601 and BT.709 should produce different R values
        let make = |coeffs: u8| -> u8 {
            let mut pic = Picture::new(2, 2, 8);
            pic.matrix_coeffs = coeffs;
            for s in pic.y_mut().iter_mut() {
                *s = 128;
            }
            for s in pic.cb_mut().iter_mut() {
                *s = 128;
            }
            for s in pic.cr_mut().iter_mut() {
                *s = 200;
            }
            let rgb = pic.to_rgb8();
            rgb[0]
        };

        let r_709 = make(MATRIX_COEFFS_BT709);
        let r_601 = make(MATRIX_COEFFS_BT601_625);
        assert_ne!(r_709, r_601);
    }

    #[test]
    fn output_length() {
        let pic = Picture::new(100, 50, 8);
        let rgb = pic.to_rgb8();
        assert_eq!(rgb.len(), 100 * 50 * 3);
    }

    #[test]
    fn single_pixel() {
        let mut pic = Picture::new(1, 1, 8);
        pic.set_y(0, 0, 200);
        pic.set_cb(0, 0, 128);
        pic.set_cr(0, 0, 128);

        let rgb = pic.to_rgb8();
        assert_eq!(rgb.len(), 3);
        assert_eq!(rgb[0], 200);
        assert_eq!(rgb[1], 200);
        assert_eq!(rgb[2], 200);
    }

    #[test]
    fn clip_u8_boundaries() {
        assert_eq!(clip_u8(-1.0), 0);
        assert_eq!(clip_u8(0.0), 0);
        assert_eq!(clip_u8(255.0), 255);
        assert_eq!(clip_u8(256.0), 255);
        assert_eq!(clip_u8(127.3), 127);
        assert_eq!(clip_u8(127.7), 128);
    }
}
