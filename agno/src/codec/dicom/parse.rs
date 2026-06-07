//! Byte-level DICOM Part-10 parser.
//!
//! Reads the 128-byte preamble + `DICM`, the Explicit-VR file-meta group (to
//! discover the transfer syntax), then walks the dataset in either Explicit or
//! Implicit VR Little Endian, skipping nested sequences to reach the pixel data.

use std::error::Error;

const PREAMBLE_LEN: usize = 128;
const UNDEFINED_LEN: u32 = 0xFFFF_FFFF;
/// Maximum sequence/item nesting depth. Real DICOM is only a few levels deep;
/// this bounds recursion so crafted deeply-nested input cannot overflow the stack.
const MAX_SEQ_DEPTH: u32 = 64;

const TS_IMPLICIT_LE: &str = "1.2.840.10008.1.2";
const TS_EXPLICIT_LE: &str = "1.2.840.10008.1.2.1";

const ITEM: Tag = Tag(0xFFFE, 0xE000);
const ITEM_DELIM: Tag = Tag(0xFFFE, 0xE00D);
const SEQ_DELIM: Tag = Tag(0xFFFE, 0xE0DD);

/// A DICOM data-element tag (group, element).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tag(pub u16, pub u16);

/// DICOM photometric interpretation (subset we render).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Photometric {
    Monochrome1,
    Monochrome2,
    Rgb,
    Other,
}

impl Photometric {
    fn from_bytes(val: &[u8]) -> Self {
        let s = String::from_utf8_lossy(val);
        match s.trim_matches(|c| c == '\0' || c == ' ') {
            "MONOCHROME1" => Photometric::Monochrome1,
            "MONOCHROME2" => Photometric::Monochrome2,
            "RGB" => Photometric::Rgb,
            _ => Photometric::Other,
        }
    }
}

/// A parsed DICOM image: rendering metadata plus a borrow of the still-encoded
/// pixel bytes (uncompressed native samples).
///
/// `Debug` is derived so `Result::unwrap_err()` works in the error-path tests.
#[derive(Debug)]
pub struct DicomImage<'a> {
    pub rows: u16,
    pub columns: u16,
    pub samples_per_pixel: u16,
    pub bits_allocated: u16,
    pub bits_stored: u16,
    pub pixel_representation: u16, // 0 = unsigned, 1 = signed
    pub planar_configuration: u16, // 0 = interleaved, 1 = planar
    pub photometric: Photometric,
    pub window_center: Option<f64>,
    pub window_width: Option<f64>,
    pub rescale_slope: f64,
    pub rescale_intercept: f64,
    pub number_of_frames: usize,
    pub pixel_data: &'a [u8],
}

/// True if `data` looks like a DICOM Part-10 file: a 128-byte preamble
/// followed by the ASCII marker `DICM`.
pub fn is_dicom(data: &[u8]) -> bool {
    data.len() >= PREAMBLE_LEN + 4 && &data[PREAMBLE_LEN..PREAMBLE_LEN + 4] == b"DICM"
}

struct Cursor<'a> {
    d: &'a [u8],
    p: usize,
}

impl<'a> Cursor<'a> {
    fn new(d: &'a [u8], p: usize) -> Self {
        Self { d, p }
    }
    fn remaining(&self) -> usize {
        self.d.len().saturating_sub(self.p)
    }
    fn u16(&mut self) -> Result<u16, Box<dyn Error>> {
        if self.remaining() < 2 {
            return Err("unexpected end of DICOM stream".into());
        }
        let v = u16::from_le_bytes([self.d[self.p], self.d[self.p + 1]]);
        self.p += 2;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, Box<dyn Error>> {
        if self.remaining() < 4 {
            return Err("unexpected end of DICOM stream".into());
        }
        let v = u32::from_le_bytes([
            self.d[self.p],
            self.d[self.p + 1],
            self.d[self.p + 2],
            self.d[self.p + 3],
        ]);
        self.p += 4;
        Ok(v)
    }
    fn tag(&mut self) -> Result<Tag, Box<dyn Error>> {
        Ok(Tag(self.u16()?, self.u16()?))
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Box<dyn Error>> {
        if self.remaining() < n {
            return Err("unexpected end of DICOM stream reading value".into());
        }
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn skip(&mut self, n: usize) -> Result<(), Box<dyn Error>> {
        if self.remaining() < n {
            return Err("unexpected end of DICOM stream skipping value".into());
        }
        self.p += n;
        Ok(())
    }
}

struct ElemHeader {
    tag: Tag,
    vr: Option<[u8; 2]>,
    length: u32,
}

/// Explicit-VR value representations that use a 4-byte length (preceded by 2
/// reserved bytes). All others use a 2-byte length.
fn is_long_vr(vr: &[u8; 2]) -> bool {
    matches!(
        vr,
        b"OB" | b"OW" | b"OF" | b"OD" | b"OL" | b"SQ" | b"UC" | b"UR" | b"UT" | b"UN"
    )
}

fn read_header(c: &mut Cursor, implicit: bool) -> Result<ElemHeader, Box<dyn Error>> {
    let tag = c.tag()?;
    if implicit {
        let length = c.u32()?;
        return Ok(ElemHeader {
            tag,
            vr: None,
            length,
        });
    }
    let vr_bytes = c.bytes(2)?;
    let vr = [vr_bytes[0], vr_bytes[1]];
    let length = if is_long_vr(&vr) {
        c.skip(2)?; // reserved
        c.u32()?
    } else {
        c.u16()? as u32
    };
    Ok(ElemHeader {
        tag,
        vr: Some(vr),
        length,
    })
}

/// Skip an undefined-length sequence: walk items until the Sequence
/// Delimitation Item (FFFE,E0DD).
fn skip_undefined_sequence(
    c: &mut Cursor,
    implicit: bool,
    depth: u32,
) -> Result<(), Box<dyn Error>> {
    if depth >= MAX_SEQ_DEPTH {
        return Err("DICOM sequence nesting too deep".into());
    }
    loop {
        let tag = c.tag()?;
        let length = c.u32()?;
        if tag == SEQ_DELIM {
            return Ok(());
        }
        if tag == ITEM {
            if length == UNDEFINED_LEN {
                skip_undefined_item(c, implicit, depth + 1)?;
            } else {
                c.skip(length as usize)?;
            }
        } else {
            return Err("malformed DICOM sequence: expected item or delimiter".into());
        }
    }
}

/// Skip an undefined-length item: walk contained elements until the Item
/// Delimitation Item (FFFE,E00D).
fn skip_undefined_item(c: &mut Cursor, implicit: bool, depth: u32) -> Result<(), Box<dyn Error>> {
    if depth >= MAX_SEQ_DEPTH {
        return Err("DICOM sequence nesting too deep".into());
    }
    loop {
        let save = c.p;
        let tag = c.tag()?;
        let _len = c.u32()?;
        if tag == ITEM_DELIM {
            return Ok(());
        }
        // Not a delimiter: rewind and parse as a normal element.
        c.p = save;
        let h = read_header(c, implicit)?;
        if h.length == UNDEFINED_LEN {
            skip_undefined_sequence(c, implicit, depth + 1)?;
        } else {
            c.skip(h.length as usize)?;
        }
    }
}

const PIXEL_DATA: Tag = Tag(0x7FE0, 0x0010);

fn read_us(val: &[u8]) -> u16 {
    if val.len() >= 2 {
        u16::from_le_bytes([val[0], val[1]])
    } else {
        0
    }
}

fn read_ds(val: &[u8]) -> Option<f64> {
    // DS may be multi-valued (backslash-separated); take the first value. Values
    // may be padded with spaces or (non-conformant) NUL; non-finite values are
    // rejected so NaN/Inf never reaches the window math.
    String::from_utf8_lossy(val)
        .split('\\')
        .next()?
        .trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

fn read_is(val: &[u8]) -> Option<i64> {
    String::from_utf8_lossy(val)
        .split('\\')
        .next()?
        .trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .parse::<i64>()
        .ok()
}

#[derive(Default)]
struct DicomBuilder {
    rows: u16,
    columns: u16,
    samples_per_pixel: u16,
    bits_allocated: u16,
    bits_stored: u16,
    pixel_representation: u16,
    planar_configuration: u16,
    photometric: Option<Photometric>,
    window_center: Option<f64>,
    window_width: Option<f64>,
    rescale_slope: Option<f64>,
    rescale_intercept: Option<f64>,
    number_of_frames: usize,
}

impl DicomBuilder {
    fn capture(&mut self, tag: Tag, val: &[u8]) {
        match tag {
            Tag(0x0028, 0x0002) => self.samples_per_pixel = read_us(val),
            Tag(0x0028, 0x0004) => self.photometric = Some(Photometric::from_bytes(val)),
            Tag(0x0028, 0x0006) => self.planar_configuration = read_us(val),
            // Record the raw frame count (clamped non-negative); build() defaults it.
            Tag(0x0028, 0x0008) => {
                self.number_of_frames = read_is(val).unwrap_or(0).max(0) as usize
            }
            Tag(0x0028, 0x0010) => self.rows = read_us(val),
            Tag(0x0028, 0x0011) => self.columns = read_us(val),
            Tag(0x0028, 0x0100) => self.bits_allocated = read_us(val),
            Tag(0x0028, 0x0101) => self.bits_stored = read_us(val),
            Tag(0x0028, 0x0103) => self.pixel_representation = read_us(val),
            Tag(0x0028, 0x1050) => self.window_center = read_ds(val),
            Tag(0x0028, 0x1051) => self.window_width = read_ds(val),
            Tag(0x0028, 0x1052) => self.rescale_intercept = read_ds(val),
            Tag(0x0028, 0x1053) => self.rescale_slope = read_ds(val),
            _ => {}
        }
    }

    fn build(self, pixel_data: &[u8]) -> Result<DicomImage<'_>, Box<dyn Error>> {
        if self.rows == 0 || self.columns == 0 {
            return Err("DICOM image has zero dimensions".into());
        }
        let bits_allocated = if self.bits_allocated == 0 {
            16
        } else {
            self.bits_allocated
        };
        if bits_allocated != 8 && bits_allocated != 16 {
            return Err(format!("unsupported DICOM BitsAllocated: {bits_allocated}").into());
        }
        Ok(DicomImage {
            rows: self.rows,
            columns: self.columns,
            samples_per_pixel: if self.samples_per_pixel == 0 {
                1
            } else {
                self.samples_per_pixel
            },
            bits_allocated,
            bits_stored: {
                let bs = if self.bits_stored == 0 {
                    bits_allocated
                } else {
                    self.bits_stored
                };
                bs.min(bits_allocated)
            },
            pixel_representation: self.pixel_representation,
            planar_configuration: self.planar_configuration,
            photometric: self.photometric.unwrap_or(Photometric::Monochrome2),
            window_center: self.window_center,
            window_width: self.window_width,
            rescale_slope: self.rescale_slope.unwrap_or(1.0),
            rescale_intercept: self.rescale_intercept.unwrap_or(0.0),
            number_of_frames: self.number_of_frames.max(1),
            pixel_data,
        })
    }
}

/// Parse a DICOM Part-10 byte stream into a [`DicomImage`].
pub fn parse_dicom(data: &[u8]) -> Result<DicomImage<'_>, Box<dyn Error>> {
    if !is_dicom(data) {
        return Err("not a DICOM Part-10 file (missing DICM marker)".into());
    }
    let mut c = Cursor::new(data, PREAMBLE_LEN + 4); // past "DICM"

    // File meta group (0002) is always Explicit VR LE. First element is
    // (0002,0000) UL = the byte length of the rest of the meta group.
    let h0 = read_header(&mut c, false)?;
    if h0.tag != Tag(0x0002, 0x0000) || h0.length != 4 {
        return Err("missing or invalid DICOM file meta group length".into());
    }
    let meta_group_len = {
        let b = c.bytes(4)?;
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize
    };
    let meta_end = c.p + meta_group_len;
    if meta_end > data.len() {
        return Err("DICOM file meta group length exceeds file size".into());
    }
    let mut transfer_syntax = String::new();
    while c.p < meta_end {
        let h = read_header(&mut c, false)?;
        if c.p + h.length as usize > meta_end {
            return Err("DICOM file meta element length exceeds meta group length".into());
        }
        let val = c.bytes(h.length as usize)?;
        if h.tag == Tag(0x0002, 0x0010) {
            transfer_syntax = String::from_utf8_lossy(val)
                .trim_matches(|ch| ch == '\0' || ch == ' ')
                .to_string();
        }
    }
    let implicit = match transfer_syntax.as_str() {
        TS_IMPLICIT_LE => true,
        TS_EXPLICIT_LE => false,
        other => {
            return Err(format!("unsupported DICOM transfer syntax: {other}").into());
        }
    };

    // Walk the dataset.
    c.p = meta_end;
    let mut b = DicomBuilder::default();
    let mut pixel_data: Option<&[u8]> = None;
    while c.remaining() > 0 {
        let h = read_header(&mut c, implicit)?;
        if h.tag == PIXEL_DATA {
            if h.length == UNDEFINED_LEN {
                return Err("encapsulated (compressed) DICOM pixel data is not supported".into());
            }
            pixel_data = Some(c.bytes(h.length as usize)?);
            break;
        }
        if h.length == UNDEFINED_LEN {
            // Undefined length implies a sequence (explicit SQ or implicit).
            skip_undefined_sequence(&mut c, implicit, 0)?;
            continue;
        }
        if !implicit && h.vr == Some(*b"SQ") {
            c.skip(h.length as usize)?;
            continue;
        }
        let val = c.bytes(h.length as usize)?;
        if h.tag.0 == 0x0028 {
            b.capture(h.tag, val);
        }
    }

    let pixel_data = pixel_data.ok_or("DICOM object has no pixel data")?;
    b.build(pixel_data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::dicom::test_fixtures::{make_part10_with_tag, pad, short, us};

    #[test]
    fn is_dicom_detects_marker() {
        let mut buf = vec![0u8; 132];
        buf[128..132].copy_from_slice(b"DICM");
        assert!(is_dicom(&buf));
        buf[130] = b'X';
        assert!(!is_dicom(&buf));
        assert!(!is_dicom(&[0u8; 10]));
    }

    #[test]
    fn skips_undefined_length_sequence() {
        // SQ (explicit) with undefined length, one undefined-length item holding
        // a single short element, then item + sequence delimiters. After skipping,
        // the cursor must land exactly on the trailing sentinel element.
        let mut data = Vec::new();
        // (0008,1110) SQ, reserved, length = 0xFFFFFFFF
        data.extend_from_slice(&0x0008u16.to_le_bytes());
        data.extend_from_slice(&0x1110u16.to_le_bytes());
        data.extend_from_slice(b"SQ");
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(&UNDEFINED_LEN.to_le_bytes());
        // Item (FFFE,E000), undefined length
        data.extend_from_slice(&ITEM.0.to_le_bytes());
        data.extend_from_slice(&ITEM.1.to_le_bytes());
        data.extend_from_slice(&UNDEFINED_LEN.to_le_bytes());
        // contained element (0008,0018) UI "1.2\0"
        data.extend_from_slice(&short(0x0008, 0x0018, b"UI", b"1.2\0"));
        // Item Delimitation (FFFE,E00D) length 0
        data.extend_from_slice(&ITEM_DELIM.0.to_le_bytes());
        data.extend_from_slice(&ITEM_DELIM.1.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        // Sequence Delimitation (FFFE,E0DD) length 0
        data.extend_from_slice(&SEQ_DELIM.0.to_le_bytes());
        data.extend_from_slice(&SEQ_DELIM.1.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        let seq_end = data.len();
        // trailing sentinel element so we can assert the landing position
        data.extend_from_slice(&short(0x0028, 0x0010, b"US", &2u16.to_le_bytes()));

        let mut c = Cursor::new(&data, 0);
        let h = read_header(&mut c, false).unwrap();
        assert_eq!(h.tag, Tag(0x0008, 0x1110));
        assert_eq!(h.length, UNDEFINED_LEN);
        skip_undefined_sequence(&mut c, false, 0).unwrap();
        assert_eq!(c.p, seq_end, "cursor must land just past the sequence");
    }

    #[test]
    fn skips_defined_length_sequence_via_byte_skip() {
        // A defined-length SQ is skipped by the caller (Task 4) with c.skip(len);
        // verify reading its header reports the right length.
        let mut data = Vec::new();
        data.extend_from_slice(&0x0008u16.to_le_bytes());
        data.extend_from_slice(&0x1032u16.to_le_bytes());
        data.extend_from_slice(b"SQ");
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(&[0u8; 8]);
        let mut c = Cursor::new(&data, 0);
        let h = read_header(&mut c, false).unwrap();
        assert_eq!(h.vr, Some(*b"SQ"));
        assert_eq!(h.length, 8);
        c.skip(h.length as usize).unwrap();
        assert_eq!(c.remaining(), 0);
    }

    // A 2x2 16-bit MONOCHROME2 dataset with a defined-length SQ in the middle to
    // prove sequence skipping is reached by the real parser.
    fn mono16_dataset() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&short(0x0028, 0x0002, b"US", &us(1))); // SamplesPerPixel
        d.extend_from_slice(&short(
            0x0028,
            0x0004,
            b"CS",
            &pad(b"MONOCHROME2".to_vec(), b' '),
        ));
        d.extend_from_slice(&short(0x0028, 0x0010, b"US", &us(2))); // Rows
        d.extend_from_slice(&short(0x0028, 0x0011, b"US", &us(2))); // Columns
        // A defined-length SQ between the geometry tags and the rest.
        d.extend_from_slice(&0x0028u16.to_le_bytes());
        d.extend_from_slice(&0x0050u16.to_le_bytes()); // arbitrary skippable tag
        d.extend_from_slice(b"SQ");
        d.extend_from_slice(&[0, 0]);
        d.extend_from_slice(&0u32.to_le_bytes()); // empty SQ
        d.extend_from_slice(&short(0x0028, 0x0100, b"US", &us(16))); // BitsAllocated
        d.extend_from_slice(&short(0x0028, 0x0101, b"US", &us(12))); // BitsStored
        d.extend_from_slice(&short(0x0028, 0x0102, b"US", &us(11))); // HighBit
        d.extend_from_slice(&short(0x0028, 0x0103, b"US", &us(0))); // PixelRepresentation
        d.extend_from_slice(&short(0x0028, 0x1050, b"DS", &pad(b"1215".to_vec(), b' '))); // WindowCenter
        d.extend_from_slice(&short(0x0028, 0x1051, b"DS", &pad(b"2113".to_vec(), b' '))); // WindowWidth
        d.extend_from_slice(&short(0x0028, 0x1052, b"DS", &pad(b"0".to_vec(), b' '))); // RescaleIntercept
        d.extend_from_slice(&short(0x0028, 0x1053, b"DS", &pad(b"2".to_vec(), b' '))); // RescaleSlope
        d
    }

    #[test]
    fn parses_mono16_metadata_and_pixels() {
        let pixel: Vec<u8> = [10u16, 20, 30, 40]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let file = make_part10_with_tag(&mono16_dataset(), (0x7FE0, 0x0010), &pixel);
        let img = parse_dicom(&file).unwrap();
        assert_eq!((img.rows, img.columns), (2, 2));
        assert_eq!(img.bits_allocated, 16);
        assert_eq!(img.bits_stored, 12);
        assert_eq!(img.samples_per_pixel, 1);
        assert_eq!(img.photometric, Photometric::Monochrome2);
        assert_eq!(img.pixel_representation, 0);
        assert_eq!(img.window_center, Some(1215.0));
        assert_eq!(img.window_width, Some(2113.0));
        assert!((img.rescale_slope - 2.0).abs() < 1e-9);
        assert!((img.rescale_intercept - 0.0).abs() < 1e-9);
        assert_eq!(img.number_of_frames, 1);
        assert_eq!(img.pixel_data, &pixel[..]);
    }

    #[test]
    fn rejects_non_dicom() {
        let err = parse_dicom(&[0u8; 64]).unwrap_err().to_string();
        assert!(err.contains("DICM"), "got: {err}");
    }

    #[test]
    fn rejects_missing_pixel_data() {
        // Use a non-pixel trailing tag so no (7FE0,0010) is present.
        let file = make_part10_with_tag(&mono16_dataset(), (0x0028, 0x0106), &[0u8; 2]);
        let err = parse_dicom(&file).unwrap_err().to_string();
        assert!(err.contains("no pixel data"), "got: {err}");
    }

    #[test]
    fn read_ds_strips_nul_padding_and_rejects_non_finite() {
        // DS/IS values padded with NUL (non-conformant but seen in the wild) must
        // still parse; "nan"/"inf" must be rejected so they never reach the window.
        assert_eq!(read_ds(b"2.5\0"), Some(2.5));
        assert_eq!(read_is(b"7\0"), Some(7));
        assert_eq!(read_ds(b"nan"), None);
        assert_eq!(read_ds(b"inf"), None);
    }

    #[test]
    fn deeply_nested_sequence_is_rejected() {
        // A dataset whose top-level undefined-length sequence is nested far deeper
        // than any real DICOM must error rather than recurse without bound.
        fn seq_body(depth: u32) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&ITEM.0.to_le_bytes());
            v.extend_from_slice(&ITEM.1.to_le_bytes());
            v.extend_from_slice(&UNDEFINED_LEN.to_le_bytes());
            if depth > 1 {
                // nested undefined-length SQ element (implicit form: tag + length)
                v.extend_from_slice(&0x0008u16.to_le_bytes());
                v.extend_from_slice(&0x1110u16.to_le_bytes());
                v.extend_from_slice(&UNDEFINED_LEN.to_le_bytes());
                v.extend_from_slice(&seq_body(depth - 1));
            }
            v.extend_from_slice(&ITEM_DELIM.0.to_le_bytes());
            v.extend_from_slice(&ITEM_DELIM.1.to_le_bytes());
            v.extend_from_slice(&0u32.to_le_bytes());
            v.extend_from_slice(&SEQ_DELIM.0.to_le_bytes());
            v.extend_from_slice(&SEQ_DELIM.1.to_le_bytes());
            v.extend_from_slice(&0u32.to_le_bytes());
            v
        }
        let mut d = Vec::new();
        d.extend_from_slice(&0x0008u16.to_le_bytes());
        d.extend_from_slice(&0x1110u16.to_le_bytes());
        d.extend_from_slice(&UNDEFINED_LEN.to_le_bytes());
        d.extend_from_slice(&seq_body(200));

        let mut out = vec![0u8; 128];
        out.extend_from_slice(b"DICM");
        let ts = pad(b"1.2.840.10008.1.2".to_vec(), 0); // Implicit VR LE
        let meta = short(0x0002, 0x0010, b"UI", &ts);
        out.extend_from_slice(&short(
            0x0002,
            0x0000,
            b"UL",
            &(meta.len() as u32).to_le_bytes(),
        ));
        out.extend_from_slice(&meta);
        out.extend_from_slice(&d);

        let err = parse_dicom(&out).unwrap_err().to_string();
        assert!(err.contains("nest"), "got: {err}");
    }

    #[test]
    fn rejects_meta_element_overshooting_group_length() {
        // A file-meta element whose declared length runs past the meta group
        // length must error clearly instead of silently consuming dataset bytes.
        let mut out = vec![0u8; 128];
        out.extend_from_slice(b"DICM");
        out.extend_from_slice(&short(0x0002, 0x0000, b"UL", &12u32.to_le_bytes()));
        // (0002,0010) UI header claiming 100 bytes — far beyond the 12-byte group.
        out.extend_from_slice(&0x0002u16.to_le_bytes());
        out.extend_from_slice(&0x0010u16.to_le_bytes());
        out.extend_from_slice(b"UI");
        out.extend_from_slice(&100u16.to_le_bytes());
        out.extend_from_slice(&[0u8; 120]); // padding so a naive read would succeed
        let err = parse_dicom(&out).unwrap_err().to_string();
        assert!(err.contains("meta"), "got: {err}");
    }

    #[test]
    fn rejects_unsupported_transfer_syntax() {
        // Hand-build a Part-10 file whose meta declares a JPEG transfer syntax.
        let mut out = vec![0u8; 128];
        out.extend_from_slice(b"DICM");
        let ts = pad(b"1.2.840.10008.1.2.4.50".to_vec(), 0); // JPEG Baseline
        let meta = short(0x0002, 0x0010, b"UI", &ts);
        out.extend_from_slice(&short(
            0x0002,
            0x0000,
            b"UL",
            &(meta.len() as u32).to_le_bytes(),
        ));
        out.extend_from_slice(&meta);
        let err = parse_dicom(&out).unwrap_err().to_string();
        assert!(err.contains("transfer syntax"), "got: {err}");
    }
}
