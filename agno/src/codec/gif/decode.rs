use std::error::Error;

use super::lzw::decompress_lzw;

const MAX_FRAMES: usize = 10000;

#[allow(clippy::type_complexity)]
pub fn decode_gif_frame(
    data: &[u8],
    frame_index: usize,
) -> Result<(Vec<u8>, u32, u32, usize), Box<dyn Error>> {
    if frame_index > MAX_FRAMES {
        return Err(format!("Frame index {frame_index} exceeds maximum {MAX_FRAMES}").into());
    }
    let mut parser = GifParser::new(data)?;
    let background = background_color(&parser.screen);
    let mut canvas = build_background_canvas(&parser.screen, background)?;
    let mut gce = GraphicsControl::default();
    let mut frames_seen = 0usize;
    // Buffer holding the previous frame's rectangle pixels, reused across frames
    // when disposal=RestorePrevious. Sized only to the painted rect, not the full canvas.
    let mut saved_rect: Vec<u8> = Vec::new();
    let mut saved_img_box: Option<FrameRect> = None;

    while let Some(img) = parser.next_image(&mut gce)? {
        let pending_gce = std::mem::take(&mut gce);

        if pending_gce.disposal == Disposal::RestorePrevious {
            save_rect(&canvas, &parser.screen, &img, &mut saved_rect);
            saved_img_box = Some(FrameRect::from_image(&img));
        }

        composite_frame(&mut canvas, &parser.screen, &img, &pending_gce)?;

        if frames_seen == frame_index {
            let total = frames_seen + 1 + parser.count_remaining_frames()?;
            return Ok((
                canvas,
                parser.screen.width as u32,
                parser.screen.height as u32,
                total,
            ));
        }

        match pending_gce.disposal {
            Disposal::RestoreBackground => {
                fill_rect(&mut canvas, &parser.screen, &img, background);
            }
            Disposal::RestorePrevious => {
                if let Some(rect) = saved_img_box {
                    copy_rect_in(&mut canvas, &parser.screen, &rect, &saved_rect);
                }
            }
            Disposal::None | Disposal::Keep => {}
        }

        frames_seen += 1;
    }
    Err(format!("GIF frame {frame_index} not found (file has {frames_seen} frames)").into())
}

pub fn gif_frame_count(data: &[u8]) -> Result<usize, Box<dyn Error>> {
    if data.len() < 6 {
        return Err("GIF file too short".into());
    }
    if &data[..6] != b"GIF87a" && &data[..6] != b"GIF89a" {
        return Err("Not a valid GIF file".into());
    }
    GifParser::new(data)?.count_remaining_frames()
}

// ---- Internals ----

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Disposal {
    #[default]
    None,
    Keep,
    RestoreBackground,
    RestorePrevious,
}

impl Disposal {
    fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Keep,
            2 => Self::RestoreBackground,
            3 => Self::RestorePrevious,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct GraphicsControl {
    disposal: Disposal,
    transparent_index: Option<u8>,
}

impl GraphicsControl {
    fn parse(payload: &[u8]) -> Self {
        if payload.len() < 4 {
            return Self::default();
        }
        let packed = payload[0];
        let disposal = Disposal::from_bits((packed >> 2) & 0x07);
        let transparent_index = if packed & 0x01 != 0 {
            Some(payload[3])
        } else {
            None
        };
        Self {
            disposal,
            transparent_index,
        }
    }
}

#[derive(Debug)]
struct LogicalScreen {
    width: u16,
    height: u16,
    global_palette: Option<Vec<u8>>, // RGB triples
    background_index: u8,
}

#[derive(Debug)]
struct ImageBlock {
    left: u16,
    top: u16,
    width: u16,
    height: u16,
    interlaced: bool,
    local_palette: Option<Vec<u8>>, // overrides global if present
    lzw_min_code_size: u8,
    lzw_payload: Vec<u8>, // concatenated sub-block bytes
}

#[derive(Clone, Copy)]
struct FrameRect {
    left: u16,
    top: u16,
    width: u16,
    height: u16,
}

impl FrameRect {
    fn from_image(img: &ImageBlock) -> Self {
        Self {
            left: img.left,
            top: img.top,
            width: img.width,
            height: img.height,
        }
    }
}

struct GifParser<'a> {
    data: &'a [u8],
    pos: usize,
    screen: LogicalScreen,
}

impl<'a> GifParser<'a> {
    fn new(data: &'a [u8]) -> Result<Self, Box<dyn Error>> {
        if data.len() < 13 {
            return Err("GIF too short for header + logical screen".into());
        }
        match &data[..6] {
            b"GIF87a" | b"GIF89a" => {}
            _ => return Err("Not a valid GIF (bad signature)".into()),
        }

        let width = u16::from_le_bytes([data[6], data[7]]);
        let height = u16::from_le_bytes([data[8], data[9]]);
        let packed = data[10];
        let background_index = data[11];
        // data[12] = pixel aspect ratio — ignore

        let mut pos = 13;
        let global_palette = if packed & 0x80 != 0 {
            let n = (packed & 0x07) as u32;
            let entries = 1usize << (n + 1);
            let bytes = entries * 3;
            if pos + bytes > data.len() {
                return Err("GIF truncated in global color table".into());
            }
            let pal = data[pos..pos + bytes].to_vec();
            pos += bytes;
            Some(pal)
        } else {
            None
        };

        Ok(Self {
            data,
            pos,
            screen: LogicalScreen {
                width,
                height,
                global_palette,
                background_index,
            },
        })
    }

    /// Advance to the next Image block, updating `gce` from any Graphics Control Extensions
    /// encountered along the way. Returns `None` at the GIF trailer.
    fn next_image(
        &mut self,
        gce: &mut GraphicsControl,
    ) -> Result<Option<ImageBlock>, Box<dyn Error>> {
        loop {
            if self.pos >= self.data.len() {
                return Err("GIF truncated before trailer".into());
            }
            let introducer = self.data[self.pos];
            self.pos += 1;
            match introducer {
                0x3B => return Ok(None),
                0x21 => {
                    if self.pos >= self.data.len() {
                        return Err("Extension truncated at label".into());
                    }
                    let label = self.data[self.pos];
                    self.pos += 1;
                    if label == 0xF9 {
                        let payload = self.read_sub_blocks()?;
                        *gce = GraphicsControl::parse(&payload);
                    } else {
                        self.skip_sub_blocks()?;
                    }
                }
                0x2C => return Ok(Some(self.parse_image_descriptor(true)?.unwrap())),
                other => return Err(format!("Unknown GIF block introducer: 0x{other:02X}").into()),
            }
        }
    }

    /// Parse an image descriptor at `self.pos - 1` (introducer already consumed).
    /// If `with_payload` is false, skip the LZW sub-blocks without allocating — used for
    /// frame counting.
    fn parse_image_descriptor(
        &mut self,
        with_payload: bool,
    ) -> Result<Option<ImageBlock>, Box<dyn Error>> {
        if self.pos + 9 > self.data.len() {
            return Err("Image descriptor truncated".into());
        }
        let left = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        let top = u16::from_le_bytes([self.data[self.pos + 2], self.data[self.pos + 3]]);
        let width = u16::from_le_bytes([self.data[self.pos + 4], self.data[self.pos + 5]]);
        let height = u16::from_le_bytes([self.data[self.pos + 6], self.data[self.pos + 7]]);
        let packed = self.data[self.pos + 8];
        self.pos += 9;
        let local_palette = if packed & 0x80 != 0 {
            let n = (packed & 0x07) as u32;
            let bytes = (1usize << (n + 1)) * 3;
            if self.pos + bytes > self.data.len() {
                return Err("GIF truncated in local color table".into());
            }
            let pal = self.data[self.pos..self.pos + bytes].to_vec();
            self.pos += bytes;
            Some(pal)
        } else {
            None
        };
        let interlaced = packed & 0x40 != 0;
        if self.pos >= self.data.len() {
            return Err("Image data truncated at LZW header".into());
        }
        let lzw_min_code_size = self.data[self.pos];
        self.pos += 1;
        if with_payload {
            let lzw_payload = self.read_sub_blocks()?;
            Ok(Some(ImageBlock {
                left,
                top,
                width,
                height,
                interlaced,
                local_palette,
                lzw_min_code_size,
                lzw_payload,
            }))
        } else {
            self.skip_sub_blocks()?;
            Ok(None)
        }
    }

    fn skip_sub_blocks(&mut self) -> Result<(), Box<dyn Error>> {
        loop {
            if self.pos >= self.data.len() {
                return Err("Sub-block stream truncated".into());
            }
            let len = self.data[self.pos] as usize;
            self.pos += 1;
            if len == 0 {
                return Ok(());
            }
            if self.pos + len > self.data.len() {
                return Err("Sub-block payload truncated".into());
            }
            self.pos += len;
        }
    }

    fn read_sub_blocks(&mut self) -> Result<Vec<u8>, Box<dyn Error>> {
        let mut out = Vec::new();
        loop {
            if self.pos >= self.data.len() {
                return Err("Sub-block stream truncated".into());
            }
            let len = self.data[self.pos] as usize;
            self.pos += 1;
            if len == 0 {
                return Ok(out);
            }
            if self.pos + len > self.data.len() {
                return Err("Sub-block payload truncated".into());
            }
            out.extend_from_slice(&self.data[self.pos..self.pos + len]);
            self.pos += len;
        }
    }

    /// Walk the remaining blocks from the current position, counting image blocks without
    /// allocating their LZW payloads. Used by `gif_frame_count` and as a cheap completion
    /// scan after decoding the target frame.
    fn count_remaining_frames(&mut self) -> Result<usize, Box<dyn Error>> {
        let mut n = 0usize;
        loop {
            if self.pos >= self.data.len() {
                return Err("GIF truncated before trailer".into());
            }
            let introducer = self.data[self.pos];
            self.pos += 1;
            match introducer {
                0x3B => return Ok(n),
                0x21 => {
                    if self.pos >= self.data.len() {
                        return Err("Extension truncated at label".into());
                    }
                    self.pos += 1;
                    self.skip_sub_blocks()?;
                }
                0x2C => {
                    self.parse_image_descriptor(false)?;
                    n += 1;
                }
                other => return Err(format!("Unknown GIF block introducer: 0x{other:02X}").into()),
            }
        }
    }
}

fn background_color(screen: &LogicalScreen) -> [u8; 3] {
    if let Some(pal) = &screen.global_palette {
        let bg = screen.background_index as usize;
        if bg * 3 + 2 < pal.len() {
            return [pal[bg * 3], pal[bg * 3 + 1], pal[bg * 3 + 2]];
        }
    }
    [0, 0, 0]
}

fn build_background_canvas(
    screen: &LogicalScreen,
    color: [u8; 3],
) -> Result<Vec<u8>, Box<dyn Error>> {
    let byte_len = (screen.width as usize)
        .checked_mul(screen.height as usize)
        .and_then(|n| n.checked_mul(3))
        .ok_or("GIF canvas dimensions overflow")?;
    let pixels = screen.width as usize * screen.height as usize;
    let mut canvas = Vec::with_capacity(byte_len);
    for _ in 0..pixels {
        canvas.extend_from_slice(&color);
    }
    Ok(canvas)
}

/// Compute the clipped frame rectangle (relative to the canvas). Returns `(cx0, cy0, w, h)`
/// in canvas coordinates, or None if the rect is entirely outside the canvas.
fn clip_rect(
    canvas_w: usize,
    canvas_h: usize,
    left: usize,
    top: usize,
    width: usize,
    height: usize,
) -> Option<(usize, usize, usize, usize)> {
    if left >= canvas_w || top >= canvas_h {
        return None;
    }
    let w = width.min(canvas_w - left);
    let h = height.min(canvas_h - top);
    if w == 0 || h == 0 {
        return None;
    }
    Some((left, top, w, h))
}

fn deinterlace_indices(width: u16, height: u16, raw: &[u8]) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; w * h];
    let passes: &[(usize, usize)] = &[(0, 8), (4, 8), (2, 4), (1, 2)];
    let mut src = 0usize;
    for &(start, step) in passes {
        let mut row = start;
        while row < h {
            out[row * w..row * w + w].copy_from_slice(&raw[src..src + w]);
            src += w;
            row += step;
        }
    }
    out
}

fn composite_frame(
    canvas: &mut [u8],
    screen: &LogicalScreen,
    img: &ImageBlock,
    gce: &GraphicsControl,
) -> Result<(), Box<dyn Error>> {
    let canvas_w = screen.width as usize;
    let canvas_h = screen.height as usize;

    let palette = img
        .local_palette
        .as_deref()
        .or(screen.global_palette.as_deref())
        .ok_or("GIF frame has no global or local color table")?;

    let frame_w = img.width as usize;
    let frame_h = img.height as usize;
    let expected = frame_w
        .checked_mul(frame_h)
        .ok_or_else(|| -> Box<dyn Error> {
            format!("GIF frame dimensions overflow: {frame_w}x{frame_h}").into()
        })?;
    let raw = decompress_lzw(img.lzw_min_code_size, &img.lzw_payload, expected)?;
    if raw.len() < expected {
        return Err(format!(
            "GIF LZW output too small: got {} bytes, expected {}",
            raw.len(),
            expected
        )
        .into());
    }
    let owned_indices: Option<Vec<u8>> = if img.interlaced {
        Some(deinterlace_indices(img.width, img.height, &raw[..expected]))
    } else {
        None
    };
    let indices: &[u8] = owned_indices.as_deref().unwrap_or(&raw[..expected]);

    let Some((cx0, cy0, clip_w, clip_h)) = clip_rect(
        canvas_w,
        canvas_h,
        img.left as usize,
        img.top as usize,
        frame_w,
        frame_h,
    ) else {
        return Ok(());
    };

    let transparent = gce.transparent_index;
    let max_pi = palette.len() / 3;

    for fy in 0..clip_h {
        let row_src = fy * frame_w;
        let row_dst = ((cy0 + fy) * canvas_w + cx0) * 3;
        for fx in 0..clip_w {
            let idx = indices[row_src + fx];
            if Some(idx) == transparent {
                continue;
            }
            let pi = idx as usize;
            if pi >= max_pi {
                return Err(format!("Palette index {pi} out of range").into());
            }
            let po = pi * 3;
            let dst = row_dst + fx * 3;
            canvas[dst] = palette[po];
            canvas[dst + 1] = palette[po + 1];
            canvas[dst + 2] = palette[po + 2];
        }
    }
    Ok(())
}

/// Fill the canvas region under `img`'s rectangle with a solid RGB color.
fn fill_rect(canvas: &mut [u8], screen: &LogicalScreen, img: &ImageBlock, color: [u8; 3]) {
    let canvas_w = screen.width as usize;
    let canvas_h = screen.height as usize;
    let Some((cx0, cy0, w, h)) = clip_rect(
        canvas_w,
        canvas_h,
        img.left as usize,
        img.top as usize,
        img.width as usize,
        img.height as usize,
    ) else {
        return;
    };
    for fy in 0..h {
        let row = ((cy0 + fy) * canvas_w + cx0) * 3;
        for fx in 0..w {
            let dst = row + fx * 3;
            canvas[dst] = color[0];
            canvas[dst + 1] = color[1];
            canvas[dst + 2] = color[2];
        }
    }
}

/// Copy the canvas rectangle under `img` into `out`, sized to exactly `w*h*3` bytes.
/// Used to snapshot the pre-paint canvas for disposal=RestorePrevious.
fn save_rect(canvas: &[u8], screen: &LogicalScreen, img: &ImageBlock, out: &mut Vec<u8>) {
    let canvas_w = screen.width as usize;
    let canvas_h = screen.height as usize;
    let Some((cx0, cy0, w, h)) = clip_rect(
        canvas_w,
        canvas_h,
        img.left as usize,
        img.top as usize,
        img.width as usize,
        img.height as usize,
    ) else {
        out.clear();
        return;
    };
    out.resize(w * h * 3, 0);
    for fy in 0..h {
        let src = ((cy0 + fy) * canvas_w + cx0) * 3;
        let dst = fy * w * 3;
        out[dst..dst + w * 3].copy_from_slice(&canvas[src..src + w * 3]);
    }
}

/// Paste a previously-saved rect buffer back into the canvas. The buffer must have been
/// produced by `save_rect` for the same `rect`.
fn copy_rect_in(canvas: &mut [u8], screen: &LogicalScreen, rect: &FrameRect, src: &[u8]) {
    let canvas_w = screen.width as usize;
    let canvas_h = screen.height as usize;
    let Some((cx0, cy0, w, h)) = clip_rect(
        canvas_w,
        canvas_h,
        rect.left as usize,
        rect.top as usize,
        rect.width as usize,
        rect.height as usize,
    ) else {
        return;
    };
    if src.len() != w * h * 3 {
        return;
    }
    for fy in 0..h {
        let dst = ((cy0 + fy) * canvas_w + cx0) * 3;
        let s = fy * w * 3;
        canvas[dst..dst + w * 3].copy_from_slice(&src[s..s + w * 3]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 2x1 single-frame GIF using the `image` crate. Returns (bytes, expected_rgb).
    fn make_single_frame_gif() -> (Vec<u8>, Vec<u8>) {
        use image::{Frame, RgbaImage, codecs::gif::GifEncoder};
        use std::io::Cursor;

        let mut rgba = RgbaImage::new(2, 1);
        rgba.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        rgba.put_pixel(1, 0, image::Rgba([0, 255, 0, 255]));

        let mut bytes = Vec::new();
        {
            let mut enc = GifEncoder::new(Cursor::new(&mut bytes));
            enc.encode_frame(Frame::new(rgba.clone())).unwrap();
        }
        // The image crate may quantize colors slightly; capture the round-tripped expected bytes.
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
        let expected = decoded.into_raw();
        (bytes, expected)
    }

    #[test]
    fn frame_count_single_frame() {
        let (bytes, _) = make_single_frame_gif();
        assert_eq!(gif_frame_count(&bytes).unwrap(), 1);
    }

    #[test]
    fn decode_single_frame_matches_image_crate() {
        let (bytes, expected_rgb) = make_single_frame_gif();
        let (rgb, w, h, count) = decode_gif_frame(&bytes, 0).unwrap();
        assert_eq!(w, 2);
        assert_eq!(h, 1);
        assert_eq!(count, 1);
        assert_eq!(rgb, expected_rgb);
    }

    #[test]
    fn decode_frame_index_out_of_range() {
        let (bytes, _) = make_single_frame_gif();
        assert!(decode_gif_frame(&bytes, 1).is_err());
    }

    #[test]
    fn invalid_signature_errors() {
        assert!(decode_gif_frame(b"NOPE89a..............", 0).is_err());
        assert!(gif_frame_count(b"NOPE89a..............").is_err());
    }

    #[test]
    fn empty_input_errors() {
        assert!(decode_gif_frame(b"", 0).is_err());
        assert!(gif_frame_count(b"").is_err());
    }

    #[test]
    fn truncated_global_color_table_errors() {
        // Header + logical screen claiming a global color table that isn't there.
        let mut buf = Vec::new();
        buf.extend_from_slice(b"GIF89a");
        buf.extend_from_slice(&2u16.to_le_bytes()); // width
        buf.extend_from_slice(&1u16.to_le_bytes()); // height
        buf.push(0xF7); // packed: GCT present, size = 7 → 256 entries → 768 bytes
        buf.push(0); // background
        buf.push(0); // aspect
        // No GCT bytes, no trailer — should error before getting to blocks.
        assert!(gif_frame_count(&buf).is_err());
    }

    /// Build a 2-frame 2x1 GIF programmatically. Frame 0 = (red, red). Frame 1 = (green, _) at offset 0.
    /// No disposal — second frame leaves the patch in place.
    fn make_two_frame_gif() -> Vec<u8> {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Frame, RgbaImage};
        use std::io::Cursor;

        let mut f0 = RgbaImage::new(2, 1);
        f0.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        f0.put_pixel(1, 0, image::Rgba([255, 0, 0, 255]));
        let mut f1 = RgbaImage::new(2, 1);
        f1.put_pixel(0, 0, image::Rgba([0, 255, 0, 255]));
        f1.put_pixel(1, 0, image::Rgba([255, 0, 0, 255]));

        let mut bytes = Vec::new();
        {
            let mut enc = GifEncoder::new(Cursor::new(&mut bytes));
            enc.set_repeat(Repeat::Infinite).unwrap();
            enc.encode_frame(Frame::new(f0)).unwrap();
            enc.encode_frame(Frame::new(f1)).unwrap();
        }
        bytes
    }

    #[test]
    fn frame_count_two_frames() {
        let bytes = make_two_frame_gif();
        assert_eq!(gif_frame_count(&bytes).unwrap(), 2);
    }

    #[test]
    fn decode_each_frame_independently() {
        let bytes = make_two_frame_gif();
        let (rgb0, w, h, total0) = decode_gif_frame(&bytes, 0).unwrap();
        assert_eq!((w, h, total0), (2, 1, 2));
        assert_eq!(rgb0, vec![255, 0, 0, 255, 0, 0]);

        let (rgb1, _, _, _) = decode_gif_frame(&bytes, 1).unwrap();
        assert_eq!(rgb1, vec![0, 255, 0, 255, 0, 0]);
    }

    #[test]
    fn decode_frame_out_of_range_multi() {
        let bytes = make_two_frame_gif();
        assert!(decode_gif_frame(&bytes, 5).is_err());
    }

    #[test]
    fn disposal_method_2_restores_background() {
        // Hand-built 2x1 2-frame GIF. Background index 0 = white. Frame 0 paints red over both pixels.
        // Frame 0's GCE has disposal=2 → before frame 1 paints, the frame 0 region is wiped to background (white).
        // Frame 1 paints green over pixel 0 only, so frame 1's canvas should be green, white.
        //
        // Layout: GIF89a header, 2x1 LSD, GCT [white, red, green], GCE+IMG, GCE+IMG, trailer.

        let mut g = Vec::new();
        g.extend_from_slice(b"GIF89a");
        g.extend_from_slice(&2u16.to_le_bytes()); // width
        g.extend_from_slice(&1u16.to_le_bytes()); // height
        // packed: GCT present (bit7=1), color resolution irrelevant, GCT size=1 → 4 entries
        g.push(0b1000_0001);
        g.push(0); // bg index = 0 (white)
        g.push(0); // aspect
        // GCT (4 entries × 3 bytes): white, red, green, black
        g.extend_from_slice(&[255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 0]);

        // GCE for frame 0: extension introducer 0x21, label 0xF9, block size 4,
        // packed: disposal=2 (010 in bits 4:2 = 0b0000_1000), no user input, no transparent color
        g.extend_from_slice(&[0x21, 0xF9, 0x04, 0b0000_1000, 0x00, 0x00, 0x00, 0x00]);
        // Image descriptor: 0x2C, left=0, top=0, w=2, h=1, packed=0 (no LCT, not interlaced)
        g.push(0x2C);
        g.extend_from_slice(&0u16.to_le_bytes());
        g.extend_from_slice(&0u16.to_le_bytes());
        g.extend_from_slice(&2u16.to_le_bytes());
        g.extend_from_slice(&1u16.to_le_bytes());
        g.push(0);
        // LZW min_code_size = 2, codes for [1, 1] = clear(4), 1, 1, eoi(5)
        // 3-bit codes, LSB-first within each code:
        //   4 = 100 → bits 0,0,1
        //   1 = 001 → bits 1,0,0
        //   1 = 001 → bits 1,0,0
        //   5 = 101 → bits 1,0,1
        // 12-bit stream (pos 0..11): 0,0,1, 1,0,0, 1,0,0, 1,0,1
        // byte0 = bits 0..7 = 0,0,1,1,0,0,1,0 → 4+8+64 = 76 = 0x4C
        // byte1 = bits 8..11+pad = 0,1,0,1,0,0,0,0 → 2+8 = 10 = 0x0A
        g.push(0x02); // min_code_size
        g.push(0x02); // sub-block length
        g.extend_from_slice(&[0x4C, 0x0A]);
        g.push(0x00); // sub-block terminator

        // GCE for frame 1: disposal=1 (do not dispose), no transparency
        g.extend_from_slice(&[0x21, 0xF9, 0x04, 0b0000_0100, 0x00, 0x00, 0x00, 0x00]);
        // Image descriptor: 0x2C, left=0, top=0, w=1, h=1, packed=0
        g.push(0x2C);
        g.extend_from_slice(&0u16.to_le_bytes());
        g.extend_from_slice(&0u16.to_le_bytes());
        g.extend_from_slice(&1u16.to_le_bytes());
        g.extend_from_slice(&1u16.to_le_bytes());
        g.push(0);
        // LZW for [2] (green): min_code_size=2, codes clear(4), 2, eoi(5)
        // 100, 010, 101 → bits low→high: 0,0,1, 0,1,0, 1,0, 1
        // byte0 = bit0=0, bit1=0, bit2=1, bit3=0, bit4=1, bit5=0, bit6=1, bit7=0 = 0b01010100 = 0x54
        // byte1 = bit0=1 → 0b00000001 = 0x01
        g.push(0x02);
        g.push(0x02);
        g.extend_from_slice(&[0x54, 0x01]);
        g.push(0x00);

        g.push(0x3B); // trailer

        // Sanity check: frame count
        assert_eq!(gif_frame_count(&g).unwrap(), 2);

        // Frame 0: both pixels red
        let (f0, _, _, _) = decode_gif_frame(&g, 0).unwrap();
        assert_eq!(f0, vec![255, 0, 0, 255, 0, 0]);

        // Frame 1: pixel 0 green (just painted), pixel 1 white (background, because disposal=2 wiped frame 0's rect)
        let (f1, _, _, _) = decode_gif_frame(&g, 1).unwrap();
        assert_eq!(f1, vec![0, 255, 0, 255, 255, 255]);
    }
}
