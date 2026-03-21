use std::backtrace::{self, Backtrace};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};

use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::agno_image::load::{ImageType, detect_image_type};
use crate::exif::spec::ExifField;
use crate::sony_decoder::{SonyDecryptor, decrypt_sr2_data};

pub mod spec;

#[allow(dead_code)]
#[repr(C)] // Ensure C-compatible layout
pub struct ExifData {
    pub data: *mut u32,
    pub len: usize,
    pub typ: u16,
}

#[allow(dead_code)]
impl ExifData {
    pub fn null() -> Self {
        ExifData {
            data: std::ptr::null_mut(),
            len: 0,
            typ: 0,
        }
    }

    fn from_bytes(bytes: &Vec<u8>, typ: u16) -> Self {
        let len = bytes.len();
        let data = unsafe { libc::malloc(len) as *mut u32 };
        if data.is_null() {
            return ExifData::null();
        }
        unsafe {
            data.copy_from_nonoverlapping(bytes.as_ptr() as *mut u32, len);
        }
        ExifData { data, len, typ }
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
                debug!(count = v.len(), "Got long value");
                let mut bytes = Vec::with_capacity(v.len() * 4);
                for n in v {
                    bytes.extend_from_slice(&n.to_le_bytes());
                }

                ExifData::from_bytes(&bytes, 4)
            }
            ExifValue::Rational(v) => {
                debug!(count = v.len(), "Got rational value");
                let mut bytes = Vec::with_capacity(v.len() * 8);
                for (num, den) in v {
                    bytes.extend_from_slice(&num.to_le_bytes());
                    bytes.extend_from_slice(&den.to_le_bytes());
                }

                ExifData::from_bytes(&bytes, 5)
            }
            ExifValue::SLong(v) => {
                debug!(count = v.len(), "Got slong value");
                let mut bytes = Vec::with_capacity(v.len() * 4);
                for n in v {
                    bytes.extend_from_slice(&n.to_le_bytes());
                }

                ExifData::from_bytes(&bytes, 9)
            }
            ExifValue::SRational(v) => {
                debug!(count = v.len(), "Got srational value");
                let mut bytes = Vec::with_capacity(v.len() * 8);
                for (num, den) in v {
                    bytes.extend_from_slice(&num.to_le_bytes());
                    bytes.extend_from_slice(&den.to_le_bytes());
                }

                ExifData::from_bytes(&bytes, 10)
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


#[derive(Debug, Clone)]
#[repr(C)] // Ensure C-compatible layout
pub struct ExifContext {
    // Where the embedded TIFF header begins (file absolute)
    tiff_base: u64,
    endian: Endian,
    // Directories we parsed (IFD0, Exif, GPS)
    exif_values: HashMap<u16, ExifValue>,
}

// ---------------- Public API ----------------

impl Default for ExifContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ExifContext {
    pub fn new() -> Self {
        ExifContext {
            tiff_base: 0,
            endian: Endian::Little,
            exif_values: HashMap::new(),
        }
    }

    pub fn from_reader_auto(reader: &mut File) -> Result<Self, ExifError> {
        let (tiff_base, endian, exif_values) = match detect_image_type(reader) {
            Ok(typ) => match typ {
                ImageType::Jpeg => Self::from_jpeg(reader)?,
                ImageType::Png => Self::from_png(reader)?,
                ImageType::Webp => return Ok(Self::new()),
                ImageType::Pdf => return Ok(Self::new()),
                ImageType::Heic => Self::from_isobmff(reader)?,
                ImageType::QuickTimeMov | ImageType::Mp4 => Self::from_mov(reader)?,
                ImageType::SonyRaw(_) => Self::from_tiff(reader, 0)?,
                ImageType::CanonRaw(_) => Self::from_tiff(reader, 0)?,
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

    // Parse EXIF from an ISOBMFF container (HEIC/HEIF/AVIF) by walking the box structure
    fn from_isobmff(reader: &mut File) -> Result<(u64, Endian, HashMap<u16, ExifValue>), ExifError> {
        let file_end = reader.seek(SeekFrom::End(0))?;

        // Find 'meta' box at top level (FullBox — 4 bytes version+flags before children)
        let (meta_start, meta_end) =
            isobmff_find_box(reader, 0, file_end, b"meta")?.ok_or(ExifError::NotExif)?;
        let children_start = meta_start + 4;

        // Find 'iinf' (item info) and 'iloc' (item location) child boxes
        let iinf_start = isobmff_find_box(reader, children_start, meta_end, b"iinf")?
            .ok_or(ExifError::NotExif)?
            .0;
        let iloc_start = isobmff_find_box(reader, children_start, meta_end, b"iloc")?
            .ok_or(ExifError::NotExif)?
            .0;

        // Find the item ID whose type is "Exif", then locate its data extent
        let exif_id =
            isobmff_find_item_id_by_type(reader, iinf_start, b"Exif")?.ok_or(ExifError::NotExif)?;
        let (offset, length) =
            isobmff_find_item_extent(reader, iloc_start, exif_id)?.ok_or(ExifError::NotExif)?;

        // Exif item data starts with a 4-byte BE offset to the TIFF header
        reader.seek(SeekFrom::Start(offset))?;
        let mut data = vec![0u8; length as usize];
        reader.read_exact(&mut data)?;

        if data.len() < 10 {
            return Err(ExifError::Malformed(
                "ISOBMFF Exif item too short".to_string(),
            ));
        }
        let tiff_off = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let start = 4 + tiff_off;
        if start >= data.len() {
            return Err(ExifError::Malformed(
                "ISOBMFF Exif TIFF offset out of bounds".to_string(),
            ));
        }

        Self::from_tiff(&mut Cursor::new(data[start..].to_vec()), 0)
    }

    /// Parse EXIF from a MOV/MP4 container.
    /// Strategy: try HEIF-style `meta` box first (iPhone MOV), then `moov/udta/meta`,
    /// then extract basic dimensions from `moov/trak/tkhd`.
    fn from_mov(reader: &mut File) -> Result<(u64, Endian, HashMap<u16, ExifValue>), ExifError> {
        // Try HEIF-style top-level meta box (works for iPhone MOV files)
        if let Ok(result) = Self::from_isobmff(reader) {
            return Ok(result);
        }

        let file_end = reader.seek(SeekFrom::End(0))?;

        // Try moov/udta/meta path
        if let Some((moov_start, moov_end)) =
            isobmff_find_box(reader, 0, file_end, b"moov")?
        {
            if let Some((udta_start, udta_end)) =
                isobmff_find_box(reader, moov_start, moov_end, b"udta")?
            {
                // Some MOV files store EXIF in a meta box under udta
                if let Some((meta_start, meta_end)) =
                    isobmff_find_box(reader, udta_start, udta_end, b"meta")?
                {
                    // FullBox: skip version + flags
                    let children_start = meta_start + 4;
                    if let (Some((iinf_start, _)), Some((iloc_start, _))) = (
                        isobmff_find_box(reader, children_start, meta_end, b"iinf")?,
                        isobmff_find_box(reader, children_start, meta_end, b"iloc")?,
                    ) {
                        if let Some(exif_id) =
                            isobmff_find_item_id_by_type(reader, iinf_start, b"Exif")?
                        {
                            if let Some((offset, length)) =
                                isobmff_find_item_extent(reader, iloc_start, exif_id)?
                            {
                                reader.seek(SeekFrom::Start(offset))?;
                                let mut data = vec![0u8; length as usize];
                                reader.read_exact(&mut data)?;
                                if data.len() >= 10 {
                                    let tiff_off = u32::from_be_bytes([
                                        data[0], data[1], data[2], data[3],
                                    ]) as usize;
                                    let start = 4 + tiff_off;
                                    if start < data.len() {
                                        return Self::from_tiff(
                                            &mut Cursor::new(data[start..].to_vec()),
                                            0,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Last resort: iterate all trak boxes to find the video track dimensions
            let mut trak_pos = moov_start;
            while trak_pos < moov_end {
                let (trak_start, trak_end) =
                    match isobmff_find_box(reader, trak_pos, moov_end, b"trak")? {
                        Some(v) => v,
                        None => break,
                    };
                trak_pos = trak_end;

                if let Some((tkhd_start, _)) =
                    isobmff_find_box(reader, trak_start, trak_end, b"tkhd")?
                {
                    // tkhd FullBox: version(1) + flags(3)
                    reader.seek(SeekFrom::Start(tkhd_start))?;
                    let mut vf = [0u8; 1];
                    reader.read_exact(&mut vf)?;
                    let version = vf[0];

                    // Skip flags(3) + creation_time + modification_time + track_id + reserved
                    let skip = if version == 0 {
                        3 + 4 + 4 + 4 + 4
                    } else {
                        3 + 8 + 8 + 4 + 4
                    };
                    reader.seek(SeekFrom::Current(skip as i64))?;

                    // Skip duration
                    reader.seek(SeekFrom::Current(if version == 0 { 4 } else { 8 }))?;

                    // Skip reserved(8) + layer(2) + alternate_group(2) + volume(2) + reserved(2) + matrix(36)
                    reader.seek(SeekFrom::Current(8 + 2 + 2 + 2 + 2 + 36))?;

                    // Width and height as 16.16 fixed-point
                    let mut dim_buf = [0u8; 8];
                    reader.read_exact(&mut dim_buf)?;
                    let width = u32::from_be_bytes([dim_buf[0], dim_buf[1], dim_buf[2], dim_buf[3]]) >> 16;
                    let height = u32::from_be_bytes([dim_buf[4], dim_buf[5], dim_buf[6], dim_buf[7]]) >> 16;

                    if width > 0 && height > 0 {
                        let values = HashMap::from([
                            (spec::IMAGE_WIDTH.tag, ExifValue::Long(vec![width])),
                            (spec::IMAGE_HEIGHT.tag, ExifValue::Long(vec![height])),
                        ]);
                        return Ok((0, Endian::Big, values));
                    }
                }
            }
        }

        // No EXIF found — return empty (same behavior as other formats)
        Ok((0, Endian::Big, HashMap::new()))
    }

    // Parse TIFF-like EXIF at tiff_base (0 for pure TIFF files, or the offset into a JPEG APP1)
    fn from_tiff<R: Read + Seek>(
        reader: &mut R,
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

        // Use worklist pattern to traverse all IFDs (like tiff.rs does)
        let mut pending_ifds = vec![ifd0_off];
        let mut visited: HashSet<u64> = HashSet::new();
        let mut exif_values = HashMap::new();

        while let Some(ifd_off) = pending_ifds.pop() {
            // Skip already-visited or invalid offsets
            if ifd_off == 0 || !visited.insert(ifd_off) {
                continue;
            }

            let ifd = match read_ifd(reader, endian, ifd_off) {
                Ok(ifd) => ifd,
                Err(_) => continue, // Skip unreadable IFDs
            };

            // Process all entries in this IFD
            for entry in &ifd.entries {
                // Queue linked IFDs before processing entry values
                // Tag 330 (0x014a) = SubIFDs (standard TIFF, can point to multiple SubIFDs)
                // Tag 0x8769 = ExifIFD
                // Tag 0x8825 = GPS IFD
                // Tag 0xc634 = DNGPrivateData (Sony SR2Private IFD)
                if entry.tag == 330
                    || entry.tag == 0x8769
                    || entry.tag == 0x8825
                    || entry.tag == 0xc634
                {
                    // Handle arrays of SubIFD offsets (tag 330 can have multiple)
                    if entry.tag == 330 && entry.typ == 4 && entry.count >= 1 {
                        if let Ok(offsets) =
                            read_long_array_tag_entry(reader, endian, tiff_base, entry)
                        {
                            for off in offsets {
                                if off != 0 {
                                    pending_ifds.push(tiff_base + off as u64);
                                }
                            }
                        }
                    } else if entry.tag == 0xc634 {
                        // DNGPrivateData - read offset from first 4 bytes of data
                        // For Sony SR2, the first 4 bytes contain a pointer to the SR2 IFD
                        if entry.typ == 1 && entry.count >= 4 {
                            // Bytes stored inline in value_or_offset for small counts
                            let off = if entry.count <= 4 {
                                entry.value_or_offset
                            } else {
                                // Read first 4 bytes from the pointed-to data
                                let data_offset = tiff_base + entry.value_or_offset as u64;
                                if reader.seek(SeekFrom::Start(data_offset)).is_ok() {
                                    let mut buf = [0u8; 4];
                                    if reader.read_exact(&mut buf).is_ok() {
                                        match endian {
                                            Endian::Little => u32::from_le_bytes(buf),
                                            Endian::Big => u32::from_be_bytes(buf),
                                        }
                                    } else {
                                        0
                                    }
                                } else {
                                    0
                                }
                            };
                            if off != 0 {
                                pending_ifds.push(tiff_base + off as u64);
                            }
                        }
                    } else if let Ok(Some(off)) =
                        read_u32_tag_if_present(reader, endian, tiff_base, &ifd, entry.tag)
                    {
                        pending_ifds.push(off);
                    }
                }

                // Tag 0x927c = MakerNotes - for Sony, this contains a TIFF-like structure
                // with "II*\0" header followed by IFD offset, then IFD entries
                if entry.tag == 0x927c && entry.typ == 7 && entry.count > 12 {
                    let makernote_offset = tiff_base + entry.value_or_offset as u64;
                    // Try to parse as Sony MakerNotes (has TIFF header)
                    if let Ok(makernote_tags) =
                        parse_sony_makernotes(reader, makernote_offset, entry.count as usize)
                    {
                        // Only add MakerNotes entries that don't conflict with existing EXIF tags
                        // MakerNotes often reuse standard tag IDs for different purposes
                        for (tag, val) in makernote_tags {
                            if !exif_values.contains_key(&tag) {
                                exif_values.insert(tag, val);
                            }
                        }
                    }
                }

                // Read entry value
                let Some(ts) = type_size(entry.typ) else {
                    continue;
                };

                let total = (entry.count as usize) * ts;

                let Ok(data) = read_value_bytes(reader, endian, tiff_base, entry, total) else {
                    continue;
                };

                let Ok(val) = read_entry_value(data, endian, entry) else {
                    continue;
                };

                exif_values.insert(entry.tag, val);
            }

            // Follow next_ifd chain
            if ifd.next_ifd != 0 {
                pending_ifds.push(tiff_base + ifd.next_ifd as u64);
            }
        }

        // Check for encrypted SR2SubIFD (Sony-specific)
        // Tags: 0x7200 = offset, 0x7201 = length, 0x7221 = decryption key
        let sr2_offset = exif_values.get(&0x7200).and_then(|v| match v {
            ExifValue::Long(vals) if !vals.is_empty() => Some(vals[0]),
            _ => None,
        });
        let sr2_length = exif_values.get(&0x7201).and_then(|v| match v {
            ExifValue::Long(vals) if !vals.is_empty() => Some(vals[0]),
            _ => None,
        });
        let sr2_key = exif_values.get(&0x7221).and_then(|v| match v {
            ExifValue::Long(vals) if !vals.is_empty() => Some(vals[0]),
            // Type 7 (UNDEFINED) stores 4 bytes inline as the key
            ExifValue::Byte(bytes) if bytes.len() >= 4 => {
                Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            }
            _ => None,
        });

        if let (Some(offset), Some(length), Some(key)) = (sr2_offset, sr2_length, sr2_key) {
            // Read encrypted SR2SubIFD data
            if let Ok(sr2_tags) =
                parse_encrypted_sr2_subifd(reader, tiff_base, offset, length, key, endian)
            {
                exif_values.extend(sr2_tags);
            }
        }

        Ok((tiff_base, endian, exif_values))
    }

    pub fn get_tag_value(&self, field: ExifField) -> Option<&ExifValue> {
        self.exif_values.get(&field.tag)
    }

    pub fn set_tag_value(&mut self, field: ExifField, value: ExifValue) {
        self.exif_values.insert(field.tag, value);
    }

    pub fn tag_count(&self) -> usize {
        self.exif_values.len()
    }

    pub fn get_tag_value_by_tag(&self, tag: u16) -> Option<&ExifValue> {
        self.exif_values.get(&tag)
    }

    /// Extract GPS coordinates as decimal degrees (latitude, longitude).
    /// Returns `None` if the required GPS tags are missing or malformed.
    pub fn get_gps_coordinates(&self) -> Option<(f64, f64)> {
        // GPS tags in the GPS sub-IFD:
        // 0x0001 = GPSLatitudeRef (ASCII: "N" or "S")
        // 0x0002 = GPSLatitude (RATIONAL x3: degrees, minutes, seconds)
        // 0x0003 = GPSLongitudeRef (ASCII: "E" or "W")
        // 0x0004 = GPSLongitude (RATIONAL x3: degrees, minutes, seconds)
        let lat_ref = match self.exif_values.get(&0x0001)? {
            ExifValue::Ascii(s) => s.trim().to_uppercase(),
            _ => return None,
        };
        let lat_rationals = match self.exif_values.get(&0x0002)? {
            ExifValue::Rational(v) if v.len() >= 3 => v,
            _ => return None,
        };
        let lon_ref = match self.exif_values.get(&0x0003)? {
            ExifValue::Ascii(s) => s.trim().to_uppercase(),
            _ => return None,
        };
        let lon_rationals = match self.exif_values.get(&0x0004)? {
            ExifValue::Rational(v) if v.len() >= 3 => v,
            _ => return None,
        };

        let to_decimal = |rats: &[(u32, u32)]| -> Option<f64> {
            if rats[0].1 == 0 || rats[1].1 == 0 || rats[2].1 == 0 {
                return None;
            }
            let deg = rats[0].0 as f64 / rats[0].1 as f64;
            let min = rats[1].0 as f64 / rats[1].1 as f64;
            let sec = rats[2].0 as f64 / rats[2].1 as f64;
            Some(deg + min / 60.0 + sec / 3600.0)
        };

        let mut lat = to_decimal(lat_rationals)?;
        let mut lon = to_decimal(lon_rationals)?;

        if lat_ref == "S" {
            lat = -lat;
        }
        if lon_ref == "W" {
            lon = -lon;
        }

        Some((lat, lon))
    }

    /// Returns all EXIF values as (name, tag, value) tuples
    #[allow(dead_code)]
    pub fn iter_all(&self) -> impl Iterator<Item = (&'static str, u16, &ExifValue)> + '_ {
        self.exif_values.iter().map(|(&tag, val)| {
            let name = spec::get_exif_field(tag)
                .map(|f| f.name)
                .unwrap_or("Unknown");
            (name, tag, val)
        })
    }
}

fn read_value_bytes<R: Read + Seek>(
    reader: &mut R,
    endian: Endian,
    tiff_base: u64,
    ent: &IfdEntry,
    want: usize,
) -> Result<Vec<u8>, ExifError> {
    let total_size = type_size(ent.typ).ok_or(ExifError::Unsupported(
        "Failed to read exif value bytes".to_string(),
    ))? * (ent.count as usize);
    if total_size <= 4 {
        // Inline in value_or_offset — must serialize back in the same
        // byte order used to parse it, so read_entry_value sees correct bytes
        let raw = match endian {
            Endian::Little => ent.value_or_offset.to_le_bytes(),
            Endian::Big => ent.value_or_offset.to_be_bytes(),
        };
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
            // value_or_offset was already parsed by read_u32_e with correct endianness
            if ent.count == 1 {
                return Ok(Some(tiff_base + ent.value_or_offset as u64));
            } else {
                // Array of LONG offsets; read first u32 from pointed area
                let ptr = tiff_base + ent.value_or_offset as u64;
                reader.seek(SeekFrom::Start(ptr))?;
                let off = read_u32_e(reader, e)?;
                return Ok(Some(tiff_base + off as u64));
            }
        } else if ent.typ == 3 && ent.count >= 1 {
            // SHORT (rare for these pointer tags, but handle)
            if ent.count == 1 {
                // For BE, the SHORT is in the upper 16 bits of the u32
                let val = match e {
                    Endian::Little => ent.value_or_offset as u16,
                    Endian::Big => (ent.value_or_offset >> 16) as u16,
                };
                return Ok(Some(tiff_base + val as u64));
            }
        }
    }
    Ok(None)
}

/// Read an array of LONG (u32) values from an IFD entry for SubIFD offsets
fn read_long_array_tag_entry<R: Read + Seek>(
    r: &mut R,
    e: Endian,
    tiff_base: u64,
    entry: &IfdEntry,
) -> Result<Vec<u32>, ExifError> {
    if entry.typ != 4 {
        return Ok(vec![]);
    }
    let count = entry.count as usize;
    let mut out = Vec::with_capacity(count);

    if count * 4 <= 4 {
        // Values fit inline in value_or_offset (only 1 value for LONG)
        out.push(entry.value_or_offset);
    } else {
        // Values stored at offset
        let ptr = tiff_base + entry.value_or_offset as u64;
        r.seek(SeekFrom::Start(ptr))?;
        for _ in 0..count {
            out.push(read_u32_e(r, e)?);
        }
    }
    Ok(out)
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

fn read_ifd<R: Read + Seek>(r: &mut R, e: Endian, ifd_abs_off: u64) -> Result<IfdInfo, ExifError> {
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

/// Parse Sony's encrypted SR2SubIFD structure
fn parse_encrypted_sr2_subifd<R: Read + Seek>(
    reader: &mut R,
    tiff_base: u64,
    offset: u32,
    length: u32,
    key: u32,
    endian: Endian,
) -> Result<HashMap<u16, ExifValue>, ExifError> {
    // Read encrypted SR2SubIFD data
    reader.seek(SeekFrom::Start(tiff_base + offset as u64))?;
    let mut sr2_data = vec![0u8; length as usize];
    reader.read_exact(&mut sr2_data)?;

    // Decrypt using Sony cipher
    let mut decryptor = SonyDecryptor::new();
    decrypt_sr2_data(&mut decryptor, &mut sr2_data, key);

    // Parse decrypted data as IFD
    parse_ifd_from_bytes(&sr2_data, endian, offset)
}

/// Parse an IFD structure from an in-memory buffer (used for decrypted SR2SubIFD)
fn parse_ifd_from_bytes(
    data: &[u8],
    endian: Endian,
    sr2_offset: u32,
) -> Result<HashMap<u16, ExifValue>, ExifError> {
    let mut result = HashMap::new();

    if data.len() < 2 {
        return Err(ExifError::Malformed(
            "SR2SubIFD too short for entry count".to_string(),
        ));
    }

    // Read entry count
    let count = match endian {
        Endian::Little => u16::from_le_bytes([data[0], data[1]]),
        Endian::Big => u16::from_be_bytes([data[0], data[1]]),
    } as usize;

    let header_size = 2; // entry count
    let entry_size = 12; // tag(2) + type(2) + count(4) + value/offset(4)
    let required_size = header_size + count * entry_size;

    if data.len() < required_size {
        return Err(ExifError::Malformed(format!(
            "SR2SubIFD too short: need {} bytes, have {}",
            required_size,
            data.len()
        )));
    }

    for i in 0..count {
        let base = header_size + i * entry_size;

        let tag = match endian {
            Endian::Little => u16::from_le_bytes([data[base], data[base + 1]]),
            Endian::Big => u16::from_be_bytes([data[base], data[base + 1]]),
        };

        let typ = match endian {
            Endian::Little => u16::from_le_bytes([data[base + 2], data[base + 3]]),
            Endian::Big => u16::from_be_bytes([data[base + 2], data[base + 3]]),
        };

        let cnt = match endian {
            Endian::Little => u32::from_le_bytes([
                data[base + 4],
                data[base + 5],
                data[base + 6],
                data[base + 7],
            ]),
            Endian::Big => u32::from_be_bytes([
                data[base + 4],
                data[base + 5],
                data[base + 6],
                data[base + 7],
            ]),
        };

        let value_or_offset = match endian {
            Endian::Little => u32::from_le_bytes([
                data[base + 8],
                data[base + 9],
                data[base + 10],
                data[base + 11],
            ]),
            Endian::Big => u32::from_be_bytes([
                data[base + 8],
                data[base + 9],
                data[base + 10],
                data[base + 11],
            ]),
        };

        let Some(ts) = type_size(typ) else {
            continue;
        };

        let total_size = (cnt as usize) * ts;

        // Get value bytes - either inline or from offset within the SR2SubIFD buffer
        let value_bytes = if total_size <= 4 {
            // Inline value
            data[base + 8..base + 8 + total_size].to_vec()
        } else {
            // Offset points relative to tiff_base, but data is within SR2SubIFD
            // The offset is relative to the TIFF base, so we need to calculate
            // the position within our decrypted buffer
            let abs_offset = value_or_offset as i64;
            let sr2_start = sr2_offset as i64;
            let rel_offset = abs_offset - sr2_start;

            if rel_offset < 0 || rel_offset as usize + total_size > data.len() {
                debug!(
                    tag = format_args!("0x{:04x}", tag),
                    offset = value_or_offset,
                    sr2_start = sr2_offset,
                    data_len = data.len(),
                    "SR2SubIFD tag offset out of bounds"
                );
                continue;
            }

            data[rel_offset as usize..rel_offset as usize + total_size].to_vec()
        };

        let entry = IfdEntry {
            tag,
            typ,
            count: cnt,
            value_or_offset,
        };

        if let Ok(val) = read_entry_value(value_bytes, endian, &entry) {
            result.insert(tag, val);
        }
    }

    Ok(result)
}

// ---------------------- ISOBMFF container helpers ----------------------
// Canonical implementations live in codec::isobmff; re-export for backward compat.

pub(crate) use crate::codec::isobmff::{
    isobmff_find_box, isobmff_find_item_extent, isobmff_find_item_id_by_type,
};

/// Parse Sony MakerNotes - starts directly with IFD entry count (no TIFF header)
/// Offsets within MakerNotes are relative to the TIFF base (file start), not MakerNotes start
fn parse_sony_makernotes<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    _length: usize,
) -> Result<HashMap<u16, ExifValue>, ExifError> {
    // Sony MakerNotes uses little-endian and offsets are relative to file start (tiff_base=0)
    let endian = Endian::Little;
    let tiff_base = 0u64;

    let mut result = HashMap::new();

    // MakerNotes starts directly with an IFD (no TIFF header)
    let ifd = read_ifd(reader, endian, offset)?;

    for entry in &ifd.entries {
        // Read entry value
        let Some(ts) = type_size(entry.typ) else {
            continue;
        };

        let total = (entry.count as usize) * ts;

        // For MakerNotes, offsets are relative to TIFF base (file start)
        let data = if total <= 4 {
            let raw = entry.value_or_offset.to_le_bytes();
            raw[..total].to_vec()
        } else {
            let abs = tiff_base + entry.value_or_offset as u64;
            reader.seek(SeekFrom::Start(abs))?;
            let mut buf = vec![0u8; total];
            reader.read_exact(&mut buf)?;
            buf
        };

        if let Ok(val) = read_entry_value(data, endian, &entry) {
            result.insert(entry.tag, val);
        }
    }

    Ok(result)
}
