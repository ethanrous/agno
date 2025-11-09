use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use log::{debug, warn};

use crate::{exif::ExifContext, sony_decoder::DecodeError};

#[derive(Clone, Copy, Debug)]
enum Endian {
    Little,
    Big,
}

#[derive(Debug, Clone)]
pub struct TiffRawInfo {
    pub make: Option<String>,
    pub model: Option<String>,
    pub dng_version: Option<u32>, // DNGVersion tag (0xC612) reduced to a u32 like 0x01000400 for 1.0.4.0
    pub width: u32,               // image width (pixels)
    pub height: u32,              // image height (pixels)
    pub bits_per_sample: u16,     // usually 12/14 (reported)
    pub compression: u16,         // 32767 for Sony custom
    pub strip_offsets: Vec<u64>,  // byte offsets to each strip
    pub strip_byte_counts: Vec<u64>, // sizes of each strip in bytes
    pub total_bytes: u64,         // sum of strip_byte_counts
    pub is_sony: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SonyVariant {
    Arw2Compressed, // block-compressed, 16-pixel blocks (bytes == width*height)
    ArwLjpeg,       // LJPEG-like differential coding with Huffman (legacy)
    Uncompressed14, // 14-bit uncompressed (bytes == width*height*2)
    Unknown,
}

pub struct TiffDetectResult {
    pub raw: TiffRawInfo,
    pub variant: SonyVariant,
}

pub fn detect_sony_raw(r: &mut File) -> Result<TiffDetectResult, DecodeError> {
    let (endian, ifd0_offset) = read_tiff_header(r)?;
    // Parse IFD0 and descend into SubIFDs to find the raw IFD with strip info
    let mut all_ifd_offsets = vec![ifd0_offset];
    let mut visited = std::collections::HashSet::new();
    let mut chosen: Option<TiffRawInfo> = None;

    let mut make: Option<String> = None;
    let mut model: Option<String> = None;
    let mut dng_version: Option<u32> = None;

    while let Some(ofs) = all_ifd_offsets.pop() {
        if !visited.insert(ofs) {
            continue;
        }
        let ifd = read_ifd(r, endian, ofs)?;

        // Extract common tags if present
        if make.is_none() {
            make = read_ascii_tag(r, endian, &ifd, 271)?; // Make
        }
        if model.is_none() {
            model = read_ascii_tag(r, endian, &ifd, 272)?; // Model
        }
        if dng_version.is_none() {
            dng_version = read_dng_version_tag(r, endian, &ifd)?;
        }

        // Enqueue SubIFDs (tag 330)
        if let Some(sub_ifds) = read_long_array_tag(r, endian, &ifd, 330)? {
            for off in sub_ifds {
                all_ifd_offsets.push(off as u64);
            }
        }

        // Check whether this IFD looks like RAW data (mono, compressed/uncompressed single plane)
        if let Some(raw_info) = try_extract_raw_info(r, endian, &ifd, &make, &model)? {
            // Choose the IFD with the largest data payload (typical RAW)
            if chosen.as_ref().map(|c| c.total_bytes).unwrap_or(0) < raw_info.total_bytes {
                chosen = Some(raw_info);
            }
        }

        // Chain to next IFD in the dir list
        let next = ifd.next_ifd;

        match next {
            Some(0) | None => {}
            Some(v) => all_ifd_offsets.push(v as u64),
        }
    }

    let mut raw = chosen.ok_or(DecodeError::CorruptData("No RAW IFD found"))?;
    raw.make = make;
    raw.model = model;
    raw.dng_version = dng_version;

    // Determine Sony variant per LibRaw’s tiff.cpp logic
    let is_sony = raw.is_sony;
    let pixels = raw.width as u64 * raw.height as u64;

    let mut variant = SonyVariant::Unknown;

    // Compression tag checks based on LibRaw behavior
    match raw.compression {
        32767 => {
            // Sony custom
            if raw.dng_version.is_none() && raw.total_bytes == pixels {
                // bytes == width*height => ARW2 block-compressed
                variant = SonyVariant::Arw2Compressed;
            } else if raw.dng_version.is_none() && is_sony && raw.total_bytes == pixels * 2 {
                // bytes == 2*width*height => 14-bit uncompressed under 32767
                variant = SonyVariant::Uncompressed14;
            } else {
                // If geometry doesn't match bps report, use ARW LJPEG shim
                // LibRaw does: if (bytes*8 != width*height*bps) { raw_height += 8; load sony_arw_load_raw; }
                let reported = pixels * raw.bits_per_sample as u64;
                if raw.total_bytes * 8 != reported {
                    variant = SonyVariant::ArwLjpeg;
                }
            }
        }
        0 | 1 => {
            // Uncompressed
            if raw.dng_version.is_none() && is_sony && raw.total_bytes == pixels * 2 {
                variant = SonyVariant::Uncompressed14;
            }
        }
        _ => {
            // Other compressions could map to LJPEG or tiles; not handled here
        }
    }

    Ok(TiffDetectResult { raw, variant })
}

// ----------------------------- Low-level TIFF parsing -----------------------------

struct Ifd {
    entries: Vec<IfdEntry>,
    next_ifd: Option<u32>,
}

#[derive(Clone)]
struct IfdEntry {
    tag: u16,
    typ: u16,
    count: u32,
    value_or_offset: u32,
}

fn read_tiff_header(r: &mut File) -> Result<(Endian, u64), DecodeError> {
    r.seek(SeekFrom::Start(0))?;
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;

    let endian = match &b[0..2] {
        b"II" => Endian::Little,
        b"MM" => Endian::Big,
        _ => return Err(DecodeError::CorruptData("Not a TIFF/ARW file")),
    };
    let u16_read = |u: &[u8]| -> u16 {
        match endian {
            Endian::Little => u16::from_le_bytes([u[0], u[1]]),
            Endian::Big => u16::from_be_bytes([u[0], u[1]]),
        }
    };
    let u32_read = |u: &[u8]| -> u32 {
        match endian {
            Endian::Little => u32::from_le_bytes([u[0], u[1], u[2], u[3]]),
            Endian::Big => u32::from_be_bytes([u[0], u[1], u[2], u[3]]),
        }
    };
    let magic = u16_read(&b[2..4]);
    if magic != 42 {
        return Err(DecodeError::CorruptData("Bad TIFF magic"));
    }
    let ifd0 = u32_read(&b[4..8]) as u64;
    Ok((endian, ifd0))
}

fn read_ifd<R: Read + Seek>(r: &mut R, e: Endian, offset: u64) -> Result<Ifd, DecodeError> {
    r.seek(SeekFrom::Start(offset))?;
    let num = read_u16_e(r, e)?;
    let mut entries = Vec::with_capacity(num as usize);
    for _ in 0..num {
        let tag = read_u16_e(r, e)?;
        let typ = read_u16_e(r, e)?;
        let count = read_u32_e(r, e)?;
        let value_or_offset = read_u32_e(r, e)?;
        entries.push(IfdEntry {
            tag,
            typ,
            count,
            value_or_offset,
        });
    }
    let next_ifd = Some(read_u32_e(r, e)?);
    Ok(Ifd { entries, next_ifd })
}

fn read_u16_e<R: Read>(r: &mut R, e: Endian) -> Result<u16, DecodeError> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(match e {
        Endian::Little => u16::from_le_bytes(b),
        Endian::Big => u16::from_be_bytes(b),
    })
}

fn read_u32_e<R: Read>(r: &mut R, e: Endian) -> Result<u32, DecodeError> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(match e {
        Endian::Little => u32::from_le_bytes(b),
        Endian::Big => u32::from_be_bytes(b),
    })
}

fn read_ascii_tag<R: Read + Seek>(
    r: &mut R,
    e: Endian,
    ifd: &Ifd,
    tag_id: u16,
) -> Result<Option<String>, DecodeError> {
    if let Some(ent) = ifd.entries.iter().find(|t| t.tag == tag_id) {
        if ent.typ != 2 || ent.count == 0 {
            return Ok(None);
        }
        let count = ent.count as usize;
        let mut buf = vec![0u8; count];
        read_tag_value_bytes(r, e, ent, &mut buf)?;
        // Trim trailing NUL if present
        if let Some(&0) = buf.last() {
            buf.pop();
        }
        if buf.is_empty() {
            return Ok(None);
        }
        let s = String::from_utf8_lossy(&buf).to_string();
        return Ok(Some(s));
    }
    Ok(None)
}

fn read_long_array_tag<R: Read + Seek>(
    r: &mut R,
    e: Endian,
    ifd: &Ifd,
    tag_id: u16,
) -> Result<Option<Vec<u32>>, DecodeError> {
    if let Some(ent) = ifd.entries.iter().find(|t| t.tag == tag_id) {
        let mut out = vec![0u32; ent.count as usize];
        read_tag_value_u32s(r, e, ent, &mut out)?;
        return Ok(Some(out));
    }
    Ok(None)
}

fn read_short_array_tag<R: Read + Seek>(
    r: &mut R,
    e: Endian,
    ifd: &Ifd,
    tag_id: u16,
) -> Result<Option<Vec<u16>>, DecodeError> {
    if let Some(ent) = ifd.entries.iter().find(|t| t.tag == tag_id) {
        if ent.typ != 3 || ent.count == 0 {
            return Ok(None);
        }
        let mut out = vec![0u16; ent.count as usize];
        read_tag_value_u16s(r, e, ent, &mut out)?;
        return Ok(Some(out));
    }
    Ok(None)
}

fn read_tag_value_bytes<R: Read + Seek>(
    r: &mut R,
    e: Endian,
    ent: &IfdEntry,
    out: &mut [u8],
) -> Result<(), DecodeError> {
    let total = out.len();
    if total <= 4 {
        // Value is inline
        let v = ent.value_or_offset.to_le_bytes(); // raw 4 bytes as stored; endian-agnostic here
        out.copy_from_slice(&v[..total]);
        Ok(())
    } else {
        r.seek(SeekFrom::Start(ent.value_or_offset as u64))?;
        r.read_exact(out)?;
        Ok(())
    }
}

fn read_tag_value_u32s<R: Read + Seek>(
    r: &mut R,
    e: Endian,
    ent: &IfdEntry,
    out: &mut [u32],
) -> Result<(), DecodeError> {
    let count = ent.count as usize;
    if count == 0 {
        return Ok(());
    }
    if count * 4 <= 4 {
        // Inline single u32
        out[0] = match e {
            Endian::Little => ent.value_or_offset,
            Endian::Big => u32::from_be_bytes(ent.value_or_offset.to_le_bytes()),
        };
        Ok(())
    } else {
        r.seek(SeekFrom::Start(ent.value_or_offset as u64))?;
        for v in out.iter_mut() {
            *v = read_u32_e(r, e)?;
        }
        Ok(())
    }
}

fn read_tag_value_u16s<R: Read + Seek>(
    r: &mut R,
    e: Endian,
    ent: &IfdEntry,
    out: &mut [u16],
) -> Result<(), DecodeError> {
    let count = ent.count as usize;
    if count == 0 {
        return Ok(());
    }
    if count * 2 <= 4 {
        // Inline up to two u16s
        let raw = ent.value_or_offset.to_le_bytes();
        for i in 0..count {
            out[i] = match e {
                Endian::Little => u16::from_le_bytes([raw[2 * i], raw[2 * i + 1]]),
                Endian::Big => u16::from_be_bytes([raw[2 * i], raw[2 * i + 1]]),
            };
        }
        Ok(())
    } else {
        r.seek(SeekFrom::Start(ent.value_or_offset as u64))?;
        for v in out.iter_mut() {
            *v = read_u16_e(r, e)?;
        }
        Ok(())
    }
}

fn try_extract_raw_info<R: Read + Seek>(
    r: &mut R,
    e: Endian,
    ifd: &Ifd,
    make: &Option<String>,
    model: &Option<String>,
) -> Result<Option<TiffRawInfo>, DecodeError> {
    // Required: width(256), height(257), compression(259), strips(273,279)

    let width = match read_long_array_tag(r, e, ifd, 256)? {
        Some(v) if !v.is_empty() => v[0],
        _ => return Ok(None),
    };

    let height = match read_long_array_tag(r, e, ifd, 257)? {
        Some(v) if !v.is_empty() => v[0],
        _ => return Ok(None),
    };
    let compression = match read_short_array_tag(r, e, ifd, 259)? {
        Some(v) if !v.is_empty() => v[0],
        _ => 1, // assume uncompressed if absent
    };
    let samples_per_pixel = match read_short_array_tag(r, e, ifd, 277)? {
        Some(v) if !v.is_empty() => v[0],
        _ => 1,
    };
    if samples_per_pixel != 1 {
        // We want the mosaic plane (1 sample per pixel)
        return Ok(None);
    }
    let bits_per_sample = match read_short_array_tag(r, e, ifd, 258)? {
        Some(v) if !v.is_empty() => v[0],
        _ => 14, // common default in ARW
    };

    let strip_offsets = match read_long_array_tag(r, e, ifd, 273)? {
        Some(v) if !v.is_empty() => v.into_iter().map(|x| x as u64).collect::<Vec<_>>(),
        _ => return Ok(None),
    };
    let strip_byte_counts = match read_long_array_tag(r, e, ifd, 279)? {
        Some(v) if !v.is_empty() => v.into_iter().map(|x| x as u64).collect::<Vec<_>>(),
        _ => return Ok(None),
    };
    if strip_offsets.len() != strip_byte_counts.len() {
        return Ok(None);
    }
    let total_bytes = strip_byte_counts.iter().sum();

    let is_sony = make
        .as_ref()
        .map(|m| m.to_ascii_lowercase().starts_with("sony"))
        .unwrap_or(false);

    Ok(Some(TiffRawInfo {
        make: None,
        model: None,
        dng_version: None,
        width,
        height,
        bits_per_sample,
        compression,
        strip_offsets,
        strip_byte_counts,
        total_bytes,
        is_sony,
    }))
}

fn read_dng_version_tag<R: Read + Seek>(
    r: &mut R,
    e: Endian,
    ifd: &Ifd,
) -> Result<Option<u32>, DecodeError> {
    // DNGVersion tag 0xC612 (50706), type BYTE, count 4
    if let Some(ent) = ifd.entries.iter().find(|t| t.tag == 0xC612) {
        if ent.typ != 1 || ent.count < 4 {
            return Ok(None);
        }
        let mut buf = vec![0u8; ent.count as usize];
        read_tag_value_bytes(r, e, ent, &mut buf)?;
        if buf.len() >= 4 {
            let v = ((buf[0] as u32) << 24)
                | ((buf[1] as u32) << 16)
                | ((buf[2] as u32) << 8)
                | (buf[3] as u32);
            return Ok(Some(v));
        }
    }
    Ok(None)
}
