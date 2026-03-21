use std::io::{Read, Seek, SeekFrom};

use crate::exif::ExifError;

/// Iterate boxes in [start, end) and return (content_start, box_end) of the first match.
pub fn isobmff_find_box<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    end: u64,
    target: &[u8; 4],
) -> Result<Option<(u64, u64)>, ExifError> {
    let mut pos = start;
    while pos + 8 <= end {
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
            (pos + 16, pos + u64::from_be_bytes(ext))
        } else if size == 0 {
            (pos + 8, end)
        } else {
            (pos + 8, pos + size as u64)
        };

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

    // Iterate child 'infe' boxes
    for _ in 0..entry_count {
        let box_pos = reader.stream_position()?;
        let mut hdr = [0u8; 8];
        if reader.read(&mut hdr)? < 8 {
            break;
        }
        let size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
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

    for _ in 0..item_count {
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
            "Invalid ISOBMFF field size: {}",
            size
        ))),
    }
}
