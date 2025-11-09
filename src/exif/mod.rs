use std::backtrace::{self, Backtrace};
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use log::debug;
use serde::{Deserialize, Serialize};

use crate::agno_image::load::{ImageType, detect_image_type};
use crate::exif::spec::ExifField;

pub mod spec;

#[repr(C)] // Ensure C-compatible layout
pub struct ExifData {
    pub data: *mut u32,
    pub len: usize,
    pub typ: u16,
}

impl ExifData {
    pub fn null() -> Self {
        ExifData {
            data: std::ptr::null_mut(),
            len: 0,
            typ: 0,
        }
    }

    fn from_bytes(bytes: &Vec<u8>) -> Self {
        let len = bytes.len();
        let data = unsafe { libc::malloc(len) as *mut u32 };
        if data.is_null() {
            return ExifData::null();
        }
        unsafe {
            data.copy_from_nonoverlapping(bytes.as_ptr() as *mut u32, len);
        }
        ExifData { data, len, typ: 0 }
    }

    pub fn from_exif_value(v: &ExifValue) -> Self {
        match v {
            ExifValue::Byte(b) => {
                let data = u16::from_le_bytes((*b.clone()).try_into().unwrap());
                ExifData {
                    data: Box::into_raw(Box::new(data as u32)),
                    len: 1,
                    typ: 1,
                }
            }
            ExifValue::Ascii(s) => ExifData {
                data: s.as_ptr() as *mut u32,
                len: s.len(),
                typ: 2,
            },
            ExifValue::Short(v) => ExifData {
                data: Box::into_raw(Box::new(v[0] as u32)),
                len: 1,
                typ: 3,
            },
            ExifValue::Long(v) => {
                debug!("Got long! {}", v.len());
                let mut bytes = Vec::with_capacity(v.len() * 4);
                for n in v {
                    bytes.extend_from_slice(&n.to_le_bytes());
                }

                ExifData::from_bytes(&bytes)
            }
            ExifValue::Rational(v) => {
                debug!("Got rational! {}", v.len());
                let mut bytes = Vec::with_capacity(v.len() * 8);
                for (num, den) in v {
                    bytes.extend_from_slice(&num.to_le_bytes());
                    bytes.extend_from_slice(&den.to_le_bytes());
                }

                ExifData::from_bytes(&bytes)
            }
            ExifValue::SLong(v) => {
                debug!("Got slong! {}", v.len());
                let mut bytes = Vec::with_capacity(v.len() * 4);
                for n in v {
                    bytes.extend_from_slice(&n.to_le_bytes());
                }

                ExifData::from_bytes(&bytes)
            }
            ExifValue::SRational(v) => {
                debug!("Got srational! {}", v.len());
                let mut bytes = Vec::with_capacity(v.len() * 8);
                for (num, den) in v {
                    bytes.extend_from_slice(&num.to_le_bytes());
                    bytes.extend_from_slice(&den.to_le_bytes());
                }

                ExifData::from_bytes(&bytes)
            }
        }
    }
}

#[derive(Debug)]
pub enum ExifError {
    Io(std::io::Error, Backtrace),
    NotExif,
    BadTiff,
    SerializeErr(serde_json::Error),
    // NotFound,
    Unsupported(String),
    Malformed(String),
}

impl std::fmt::Display for ExifError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ExifError::Io(e, bt) => write!(f, "I/O error: {}\n{}", e, bt),
            ExifError::NotExif => write!(f, "Not a valid EXIF/JPEG file"),
            ExifError::BadTiff => write!(f, "Invalid TIFF header in EXIF data"),
            ExifError::SerializeErr(e) => write!(f, "Serialization error: {}", e),
            // ExifError::NotFound => write!(f, "Requested EXIF tag not found"),
            ExifError::Unsupported(msg) => {
                write!(f, "Unsupported EXIF data type or format: {}", msg)
            }
            ExifError::Malformed(msg) => write!(f, "Malformed EXIF data: {}", msg),
        }
    }
}

impl From<std::io::Error> for ExifError {
    fn from(err: std::io::Error) -> ExifError {
        ExifError::Io(err, backtrace::Backtrace::capture())
    }
}

impl From<serde_json::Error> for ExifError {
    fn from(err: serde_json::Error) -> ExifError {
        ExifError::SerializeErr(err)
    }
}

impl std::error::Error for ExifError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

#[derive(Clone, Copy, Debug)]
enum Endian {
    Little,
    Big,
}

#[derive(Clone, Debug)]
struct IfdEntry {
    tag: u16,
    typ: u16,
    count: u32,
    value_or_offset: u32,
}

#[derive(Clone, Debug)]
struct IfdInfo {
    entries: Vec<IfdEntry>,
    next_ifd: u32, // offset to next IFD (0 if none)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum ExifValue {
    Byte(Vec<u8>),              // BYTE(1) or UNDEFINED(7)
    Ascii(String),              // ASCII(2)
    Short(Vec<u16>),            // SHORT(3)
    Long(Vec<u32>),             // LONG(4)
    Rational(Vec<(u32, u32)>),  // RATIONAL(5)
    SLong(Vec<i32>),            // SLONG(9)
    SRational(Vec<(i32, i32)>), // SRATIONAL(10)
}

pub struct ExifKVPair {
    pub name: String,
    pub value: ExifValue,
}

impl fmt::Display for ExifKVPair {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let name = &self.name;
        match &self.value {
            ExifValue::Ascii(s) => write!(f, "{name:<30}: {value}", value = s),

            ExifValue::Rational(v) if !v.is_empty() => {
                let (num, den) = v[0];
                write!(f, "{name:<30}: {num}/{den}")
            }
            ExifValue::SRational(v) if !v.is_empty() => {
                let (num, den) = v[0];
                write!(f, "{name:<30}: {num}/{den} (signed)")
            }
            ExifValue::Short(v) if !v.is_empty() => write!(f, "{name:<30}: {}", v[0]),
            ExifValue::Long(v) if !v.is_empty() => write!(f, "{name:<30}: {}", v[0]),
            ExifValue::Byte(v) => {
                write!(
                    f,
                    "{name:<30}: {:?} ({} bytes)",
                    &v[..v.len().min(8)],
                    v.len()
                )
            }
            _ => write!(f, "{name:<30}: <value>"),
        }
    }
}

trait ReadSeeker: Read + Seek {}

#[derive(Debug, Clone)]
#[repr(C)] // Ensure C-compatible layout
pub struct ExifContext {
    // Where the embedded TIFF header begins (file absolute)
    tiff_base: u64,
    endian: Endian,
    // Directories we parsed (IFD0, Exif, GPS)
    exif_values: HashMap<u16, ExifValue>,
}

#[derive(Serialize, Deserialize)]
struct ExifMap {
    map: HashMap<String, ExifValue>,
}

// ---------------- Public API ----------------

impl ExifContext {
    pub fn new() -> Self {
        ExifContext {
            tiff_base: 0,
            endian: Endian::Little,
            exif_values: HashMap::new(),
        }
    }

    pub fn from_path_auto(path: &str) -> Result<Self, ExifError> {
        let mut file = File::open(path)?;
        ExifContext::from_reader_auto(&mut file)
    }

    pub fn from_reader_auto(reader: &mut File) -> Result<Self, ExifError> {
        let (tiff_base, endian, exif_values) = match detect_image_type(reader) {
            Ok(typ) => match typ {
                ImageType::Jpeg => Self::from_jpeg(reader)?,
                ImageType::Png => Self::from_png(reader)?,
                ImageType::Webp => return Ok(Self::new()),
                ImageType::Pdf => return Ok(Self::new()),
                ImageType::SonyRaw(_) => Self::from_tiff(reader, 0)?,
            },
            Err(e) => {
                return Err(ExifError::Unsupported(format!(
                    "No matching EXIF parser: {}",
                    e
                )));
            }
        };

        Ok(ExifContext {
            tiff_base,
            endian,
            exif_values,
        })
    }

    // Parse EXIF from a JPEG APP1 Exif segment
    fn from_jpeg(reader: &mut File) -> Result<(u64, Endian, HashMap<u16, ExifValue>), ExifError> {
        reader.seek(SeekFrom::Start(0))?;
        // SOI
        let soi = read_exact_vec(reader, 2)?;
        if soi != [0xFF, 0xD8] {
            return Err(ExifError::NotExif);
        }
        loop {
            // Scan for next marker 0xFFxx (skip fill 0xFF)
            let mut b = [0u8; 1];
            // Find 0xFF
            loop {
                if reader.read(&mut b)? == 0 {
                    return Err(ExifError::NotExif);
                }
                if b[0] == 0xFF {
                    break;
                }
            }
            // Read marker type (skip multiple 0xFF)
            let mut m = [0u8; 1];
            loop {
                reader.read_exact(&mut m)?;
                if m[0] != 0xFF {
                    break;
                }
            }
            let marker = m[0];
            if marker == 0xD9 || marker == 0xDA {
                // EOI or SOS -> stop search
                return Err(ExifError::NotExif);
            }
            // Read segment length (big-endian), includes length bytes
            let seg_len = read_be_u16(reader)?;
            if marker == 0xE1 {
                // APP1
                let header = read_exact_vec(reader, 6)?;
                if &header == b"Exif\0\0" {
                    // TIFF data starts here
                    let tiff_base = reader.stream_position()?; // after "Exif\0\0"
                    return Self::from_tiff(reader, tiff_base);
                } else {
                    // Skip rest of APP1
                    let to_skip = seg_len as i64 - 2 - 6;
                    if to_skip > 0 {
                        reader.seek(SeekFrom::Current(to_skip))?;
                    }
                }
            } else {
                // Skip this segment
                let to_skip = seg_len as i64 - 2;
                if to_skip > 0 {
                    reader.seek(SeekFrom::Current(to_skip))?;
                }
            }
        }
    }

    fn from_png(reader: &mut File) -> Result<(u64, Endian, HashMap<u16, ExifValue>), ExifError> {
        // Reset to start and verify PNG signature
        reader.seek(SeekFrom::Start(0))?;
        let mut sig = [0u8; 16];
        reader.read_exact(&mut sig)?;
        if sig
            != [
                0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
                0x44, 0x52,
            ]
        {
            return Err(ExifError::Malformed(
                "Not a valid PNG file, signature mismatch".to_string(),
            ));
        }

        let mut size_buf = [0u8; 8];
        reader.read_exact(&mut size_buf)?;

        let width = u32::from_be_bytes(size_buf[0..4].try_into().unwrap());
        let height = u32::from_be_bytes(size_buf[4..8].try_into().unwrap());

        let values = HashMap::from([
            (spec::IMAGE_WIDTH.tag, ExifValue::Long(vec![width])),
            (spec::IMAGE_HEIGHT.tag, ExifValue::Long(vec![height])),
        ]);

        return Ok((0, Endian::Big, values));
    }

    // Parse TIFF-like EXIF at tiff_base (0 for pure TIFF files, or the offset into a JPEG APP1)
    fn from_tiff(
        reader: &mut File,
        tiff_base: u64,
    ) -> Result<(u64, Endian, HashMap<u16, ExifValue>), ExifError> {
        // Read TIFF header (II/MM, 42, offset to IFD0)
        reader.seek(SeekFrom::Start(tiff_base))?;
        let endian = match read_exact_vec(reader, 2)?.as_slice() {
            b"II" => Endian::Little,
            b"MM" => Endian::Big,
            _ => return Err(ExifError::BadTiff),
        };

        let magic = read_u16_e(reader, endian)?;
        if magic != 42 {
            return Err(ExifError::BadTiff);
        }
        let ifd0_off_rel = read_u32_e(reader, endian)? as u64;
        let ifd0_off = tiff_base + ifd0_off_rel;

        // Parse IFD0
        let ifd0 = read_ifd(reader, endian, ifd0_off)?;
        // Read Make(0x010F), Model(0x0110) if present

        // Find ExifIFD (0x8769) and GPS (0x8825) offsets
        let exif_off = read_u32_tag_if_present(reader, endian, tiff_base, &ifd0, 0x8769)
            .ok()
            .flatten();

        let sub_ifd_off = read_u32_tag_if_present(reader, endian, tiff_base, &ifd0, 0x014a)
            .ok()
            .flatten();
        // let gps_off = read_u32_tag_if_present(reader, endian, tiff_base, &ifd0, 0x8825)
        //     .ok()
        //     .flatten();

        let mut sects = vec![ifd0];

        match sub_ifd_off {
            Some(0) | None => {}
            Some(off) => {
                let sub_ifd = read_ifd(reader, endian, off)?;
                sects.push(sub_ifd);
            }
        }

        match exif_off {
            Some(0) | None => {}
            Some(off) => {
                let exif_ifd = read_ifd(reader, endian, off)?;
                sects.push(exif_ifd);
            }
        }

        match sub_ifd_off {
            Some(0) | None => {}
            Some(off) => {
                let sub_ifd = read_ifd(reader, endian, off)?;
                sects.push(sub_ifd);
            }
        }

        let mut exif_values = HashMap::new();

        sects.iter().for_each(|info| {
            info.entries.iter().for_each(|entry| {
                let Ok(ts) = type_size(entry.typ).ok_or(ExifError::Unsupported) else {
                    return;
                };

                let total = (entry.count as usize) * ts;

                let Ok(data) = read_value_bytes(reader, tiff_base, entry, total) else {
                    return;
                };

                let Ok(val) = read_entry_value(data, endian, entry) else {
                    return;
                };

                exif_values.insert(entry.tag, val);
            });
        });

        Ok((tiff_base, endian, exif_values))
    }

    pub fn to_json(&mut self) -> Result<String, ExifError> {
        let s = serde_json::to_string(&self.exif_values)?;
        Ok(s)
    }

    pub fn get_tag_value(&self, field: ExifField) -> Option<&ExifValue> {
        self.exif_values.get(&field.tag)
    }

    pub fn get_tag_value_by_tag(&self, tag: u16) -> Option<&ExifValue> {
        self.exif_values.get(&tag)
    }
}

fn read_value_bytes(
    reader: &mut File,
    tiff_base: u64,
    ent: &IfdEntry,
    want: usize,
) -> Result<Vec<u8>, ExifError> {
    let total_size = type_size(ent.typ).ok_or(ExifError::Unsupported(
        "Failed to read exif value bytes".to_string(),
    ))? * (ent.count as usize);
    if total_size <= 4 {
        // Inline in value_or_offset (endian-agnostic raw bytes)
        let raw = ent.value_or_offset.to_le_bytes();
        Ok(raw[..total_size.min(want)].to_vec())
    } else {
        let abs = tiff_base + ent.value_or_offset as u64;
        reader.seek(SeekFrom::Start(abs))?;
        let mut buf = vec![0u8; want.min(total_size)];
        reader.read_exact(&mut buf)?;
        Ok(buf)
    }
}

fn read_entry_value(data: Vec<u8>, e: Endian, ent: &IfdEntry) -> Result<ExifValue, ExifError> {
    match ent.typ {
        1 | 7 => Ok(ExifValue::Byte(data)), // BYTE or UNDEFINED
        2 => {
            // ASCII (NUL-terminated)
            let mut s = data;
            if let Some(&0) = s.last() {
                s.pop();
            }
            Ok(ExifValue::Ascii(String::from_utf8_lossy(&s).to_string()))
        }
        3 | 8 => {
            // SHORT
            let mut v = Vec::with_capacity(ent.count as usize);
            for i in 0..(ent.count as usize) {
                let off = i * 2;
                let n = match e {
                    Endian::Little => u16::from_le_bytes([data[off], data[off + 1]]),
                    Endian::Big => u16::from_be_bytes([data[off], data[off + 1]]),
                };
                v.push(n);
            }
            Ok(ExifValue::Short(v))
        }
        4 => {
            // LONG
            let mut v = Vec::with_capacity(ent.count as usize);
            for i in 0..(ent.count as usize) {
                let off = i * 4;
                let n = match e {
                    Endian::Little => {
                        u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    }
                    Endian::Big => {
                        u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    }
                };
                v.push(n);
            }
            Ok(ExifValue::Long(v))
        }
        5 => {
            // RATIONAL (u32/u32)
            let mut v = Vec::with_capacity(ent.count as usize);
            for i in 0..(ent.count as usize) {
                let off = i * 8;
                let num = match e {
                    Endian::Little => {
                        u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    }
                    Endian::Big => {
                        u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    }
                };
                let den = match e {
                    Endian::Little => u32::from_le_bytes([
                        data[off + 4],
                        data[off + 5],
                        data[off + 6],
                        data[off + 7],
                    ]),
                    Endian::Big => u32::from_be_bytes([
                        data[off + 4],
                        data[off + 5],
                        data[off + 6],
                        data[off + 7],
                    ]),
                };
                v.push((num, den));
            }
            Ok(ExifValue::Rational(v))
        }
        9 => {
            // SLONG (i32)
            let mut v = Vec::with_capacity(ent.count as usize);
            for i in 0..(ent.count as usize) {
                let off = i * 4;
                let n = match e {
                    Endian::Little => {
                        i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    }
                    Endian::Big => {
                        i32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    }
                };
                v.push(n);
            }
            Ok(ExifValue::SLong(v))
        }
        10 => {
            // SRATIONAL (i32/i32)
            let mut v = Vec::with_capacity(ent.count as usize);
            for i in 0..(ent.count as usize) {
                let off = i * 8;
                let num = match e {
                    Endian::Little => {
                        i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    }
                    Endian::Big => {
                        i32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
                    }
                };
                let den = match e {
                    Endian::Little => i32::from_le_bytes([
                        data[off + 4],
                        data[off + 5],
                        data[off + 6],
                        data[off + 7],
                    ]),
                    Endian::Big => i32::from_be_bytes([
                        data[off + 4],
                        data[off + 5],
                        data[off + 6],
                        data[off + 7],
                    ]),
                };
                v.push((num, den));
            }
            Ok(ExifValue::SRational(v))
        }
        _ => Err(ExifError::Unsupported("Unknown EXIF data type".to_string())),
    }
}

// ---------------------- TIFF helpers and value decoding ------------------------
fn read_u32_tag_if_present<R: Read + Seek>(
    reader: &mut R,
    e: Endian,
    tiff_base: u64,
    ifd: &IfdInfo,
    tag: u16,
) -> Result<Option<u64>, ExifError> {
    if let Some(ent) = ifd.entries.iter().find(|e2| e2.tag == tag) {
        if ent.typ == 4 && ent.count >= 1 {
            // value_or_offset is inline if count*4 <= 4, otherwise points to array
            if ent.count == 1 {
                // Inline or offset: for count==1 LONG, spec stores inline.
                let off = match e {
                    Endian::Little => ent.value_or_offset,
                    Endian::Big => u32::from_be_bytes(ent.value_or_offset.to_le_bytes()),
                };
                return Ok(Some(tiff_base + off as u64));
            } else {
                // Array of LONG offsets; we only take the first as ExifIFD starts there
                // Read first u32 from pointed area
                let ptr = tiff_base + ent.value_or_offset as u64;
                reader.seek(SeekFrom::Start(ptr))?;
                let off = read_u32_e(reader, e)?;
                return Ok(Some(tiff_base + off as u64));
            }
        } else if ent.typ == 3 && ent.count >= 1 {
            // SHORT (rare for these pointer tags, but handle)
            if ent.count == 1 {
                let val = match e {
                    Endian::Little => ent.value_or_offset as u16,
                    Endian::Big => u16::from_be_bytes((ent.value_or_offset as u16).to_le_bytes()),
                };
                return Ok(Some(tiff_base + val as u64));
            }
        }
    }
    Ok(None)
}

#[inline(never)]
fn read_exact_vec<R: Read>(r: &mut R, n: usize) -> Result<Vec<u8>, ExifError> {
    let mut v = vec![0u8; n];
    r.read_exact(&mut v)?;
    Ok(v)
}

fn read_u16_e<R: Read>(r: &mut R, e: Endian) -> Result<u16, ExifError> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(match e {
        Endian::Little => u16::from_le_bytes(b),
        Endian::Big => u16::from_be_bytes(b),
    })
}

fn read_u32_e<R: Read>(r: &mut R, e: Endian) -> Result<u32, ExifError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(match e {
        Endian::Little => u32::from_le_bytes(b),
        Endian::Big => u32::from_be_bytes(b),
    })
}

fn read_be_u16<R: Read>(r: &mut R) -> Result<u16, ExifError> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

fn read_ifd(r: &mut File, e: Endian, ifd_abs_off: u64) -> Result<IfdInfo, ExifError> {
    r.seek(SeekFrom::Start(ifd_abs_off))?;
    let count = read_u16_e(r, e)? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let tag = read_u16_e(r, e)?;
        let typ = read_u16_e(r, e)?;
        let cnt = read_u32_e(r, e)?;
        let valoff = read_u32_e(r, e)?;
        entries.push(IfdEntry {
            tag,
            typ,
            count: cnt,
            value_or_offset: valoff,
        });
    }
    let next = read_u32_e(r, e)?;
    Ok(IfdInfo {
        entries,
        next_ifd: next,
    })
}

fn type_size(typ: u16) -> Option<usize> {
    match typ {
        1 => Some(1),  // BYTE
        2 => Some(1),  // ASCII
        3 => Some(2),  // SHORT
        4 => Some(4),  // LONG
        5 => Some(8),  // RATIONAL
        7 => Some(1),  // UNDEFINED
        8 => Some(2),  // SSHORT
        9 => Some(4),  // SLONG
        10 => Some(8), // SRATIONAL
        _ => None,
    }
}
