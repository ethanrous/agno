use std::io::{Read, Seek, SeekFrom};

use crate::exif::ExifError;

// Security limits ported from libheif (security_limits.h / security_limits.cc).
// These cap resource consumption when parsing untrusted HEIF containers.

/// Maximum number of items allowed in iloc/iinf boxes.
const MAX_ITEMS: usize = 10_000;

/// Maximum nesting depth (iteration count) when scanning sibling boxes.
/// Prevents stack-like exhaustion on deeply nested or cyclic box structures.
const MAX_BOX_ITERATIONS: usize = 2_048;

/// Maximum number of extents per item in an iloc box.
const MAX_EXTENTS_PER_ITEM: usize = 1_000;

/// Format a 4-byte box type as a printable string.
/// Non-ASCII bytes are replaced with '?' so the string is always safe to embed in errors.
fn box_type_str(b: &[u8]) -> String {
    b.iter()
        .map(|&c| if c.is_ascii_graphic() || c == b' ' { c as char } else { '?' })
        .collect()
}

/// Iterate boxes in [start, end) and return (content_start, box_end) of the first match.
pub fn isobmff_find_box<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    end: u64,
    target: &[u8; 4],
) -> Result<Option<(u64, u64)>, ExifError> {
    let mut pos = start;
    let mut iterations = 0usize;
    while pos + 8 <= end {
        iterations += 1;
        if iterations > MAX_BOX_ITERATIONS {
            return Err(ExifError::Malformed(format!(
                "Exceeded {} box iterations searching for '{}' in range [{:#x}, {:#x})",
                MAX_BOX_ITERATIONS, box_type_str(target), start, end,
            )));
        }

        reader.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        if reader.read(&mut hdr)? < 8 {
            break;
        }
        let size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let box_type = &hdr[4..8];

        let (content_start, box_end) = if size == 1 {
            let mut ext = [0u8; 8];
            reader.read_exact(&mut ext)?;
            let large_size = u64::from_be_bytes(ext);
            if large_size < 16 {
                return Err(ExifError::Malformed(format!(
                    "Box '{}' at offset {:#x}: extended size {} is smaller than header",
                    box_type_str(box_type), pos, large_size,
                )));
            }
            (pos + 16, pos + large_size)
        } else if size == 0 {
            (pos + 8, end)
        } else {
            if (size as u64) < 8 {
                return Err(ExifError::Malformed(format!(
                    "Box '{}' at offset {:#x}: size {} is smaller than box header",
                    box_type_str(box_type), pos, size,
                )));
            }
            (pos + 8, pos + size as u64)
        };

        if box_end > end {
            return Err(ExifError::Malformed(format!(
                "Box '{}' at offset {:#x}: end {:#x} exceeds container boundary {:#x}",
                box_type_str(box_type), pos, box_end, end,
            )));
        }

        if box_type == target {
            return Ok(Some((content_start, box_end)));
        }
        if box_end <= pos {
            break; // prevent infinite loop on malformed data
        }
        pos = box_end;
    }
    Ok(None)
}

/// Scan 'infe' entries inside an 'iinf' FullBox and return the item ID with the given type.
pub fn isobmff_find_item_id_by_type<R: Read + Seek>(
    reader: &mut R,
    iinf_content_start: u64,
    target_type: &[u8; 4],
) -> Result<Option<u32>, ExifError> {
    reader.seek(SeekFrom::Start(iinf_content_start))?;
    // FullBox header: version(1) + flags(3)
    let mut vf = [0u8; 4];
    reader.read_exact(&mut vf)?;
    let version = vf[0];

    let entry_count = if version == 0 {
        let mut buf = [0u8; 2];
        reader.read_exact(&mut buf)?;
        u16::from_be_bytes(buf) as u32
    } else {
        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf)?;
        u32::from_be_bytes(buf)
    };

    if entry_count as usize > MAX_ITEMS {
        return Err(ExifError::Malformed(format!(
            "iinf box at offset {:#x}: entry count {} exceeds security limit of {}",
            iinf_content_start, entry_count, MAX_ITEMS,
        )));
    }

    // Iterate child 'infe' boxes
    for i in 0..entry_count {
        let box_pos = reader.stream_position()?;
        let mut hdr = [0u8; 8];
        if reader.read(&mut hdr)? < 8 {
            break;
        }
        let size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        if size < 8 {
            return Err(ExifError::Malformed(format!(
                "infe box {} at offset {:#x}: size {} is smaller than box header",
                i, box_pos, size,
            )));
        }
        let box_end = box_pos + size;

        if &hdr[4..8] == b"infe" {
            // FullBox: version(1) + flags(3)
            let mut vf2 = [0u8; 4];
            reader.read_exact(&mut vf2)?;
            let infe_version = vf2[0];

            if infe_version >= 2 {
                let item_id = if infe_version == 2 {
                    let mut buf = [0u8; 2];
                    reader.read_exact(&mut buf)?;
                    u16::from_be_bytes(buf) as u32
                } else {
                    let mut buf = [0u8; 4];
                    reader.read_exact(&mut buf)?;
                    u32::from_be_bytes(buf)
                };

                // Skip item_protection_index (2 bytes)
                reader.seek(SeekFrom::Current(2))?;

                let mut item_type = [0u8; 4];
                reader.read_exact(&mut item_type)?;

                if &item_type == target_type {
                    return Ok(Some(item_id));
                }
            }
        }

        if box_end <= box_pos {
            break;
        }
        reader.seek(SeekFrom::Start(box_end))?;
    }
    Ok(None)
}

/// Parse an 'iloc' FullBox to find the first extent (offset, length) for the given item ID.
pub fn isobmff_find_item_extent<R: Read + Seek>(
    reader: &mut R,
    iloc_content_start: u64,
    target_id: u32,
) -> Result<Option<(u64, u64)>, ExifError> {
    reader.seek(SeekFrom::Start(iloc_content_start))?;
    let mut vf = [0u8; 4];
    reader.read_exact(&mut vf)?;
    let version = vf[0];

    // Packed size fields: offset_size(4) | length_size(4) | base_offset_size(4) | index_size(4)
    let mut sizes = [0u8; 2];
    reader.read_exact(&mut sizes)?;
    let offset_size = ((sizes[0] >> 4) & 0xF) as usize;
    let length_size = (sizes[0] & 0xF) as usize;
    let base_offset_size = ((sizes[1] >> 4) & 0xF) as usize;
    let index_size = if version >= 1 {
        (sizes[1] & 0xF) as usize
    } else {
        0
    };

    let item_count = if version < 2 {
        let mut buf = [0u8; 2];
        reader.read_exact(&mut buf)?;
        u16::from_be_bytes(buf) as u32
    } else {
        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf)?;
        u32::from_be_bytes(buf)
    };

    if item_count as usize > MAX_ITEMS {
        return Err(ExifError::Malformed(format!(
            "iloc box at offset {:#x}: item count {} exceeds security limit of {}",
            iloc_content_start, item_count, MAX_ITEMS,
        )));
    }

    for _ in 0..item_count {
        let item_pos = reader.stream_position()?;
        let item_id = if version < 2 {
            let mut buf = [0u8; 2];
            reader.read_exact(&mut buf)?;
            u16::from_be_bytes(buf) as u32
        } else {
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            u32::from_be_bytes(buf)
        };

        // construction_method (v1/v2 only)
        if version >= 1 {
            reader.seek(SeekFrom::Current(2))?;
        }
        // data_reference_index
        reader.seek(SeekFrom::Current(2))?;

        let base_offset = isobmff_read_uint(reader, base_offset_size)?;

        let mut ec = [0u8; 2];
        reader.read_exact(&mut ec)?;
        let extent_count = u16::from_be_bytes(ec);

        if extent_count as usize > MAX_EXTENTS_PER_ITEM {
            return Err(ExifError::Malformed(format!(
                "iloc box: item {} at offset {:#x} has {} extents, exceeds security limit of {}",
                item_id, item_pos, extent_count, MAX_EXTENTS_PER_ITEM,
            )));
        }

        for _ in 0..extent_count {
            if version >= 1 && index_size > 0 {
                isobmff_read_uint(reader, index_size)?;
            }
            let extent_offset = isobmff_read_uint(reader, offset_size)?;
            let extent_length = isobmff_read_uint(reader, length_size)?;

            if item_id == target_id {
                return Ok(Some((base_offset + extent_offset, extent_length)));
            }
        }
    }
    Ok(None)
}

pub fn isobmff_read_uint<R: Read>(reader: &mut R, size: usize) -> Result<u64, ExifError> {
    match size {
        0 => Ok(0),
        2 => {
            let mut buf = [0u8; 2];
            reader.read_exact(&mut buf)?;
            Ok(u16::from_be_bytes(buf) as u64)
        }
        4 => {
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            Ok(u32::from_be_bytes(buf) as u64)
        }
        8 => {
            let mut buf = [0u8; 8];
            reader.read_exact(&mut buf)?;
            Ok(u64::from_be_bytes(buf))
        }
        _ => Err(ExifError::Malformed(format!(
            "Invalid ISOBMFF field size: {} (expected 0, 2, 4, or 8)",
            size,
        ))),
    }
}
