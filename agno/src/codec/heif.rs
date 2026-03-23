use std::io::{Read, Seek, SeekFrom};

use anyhow::{Context, Result, bail, ensure};

use super::isobmff::{isobmff_find_box, isobmff_find_item_extent};

/// Maximum number of tiles in a grid image.
const MAX_TILES: usize = 16_384;

/// Maximum byte size for a single tile bitstream (256 MB).
const MAX_TILE_DATA_SIZE: u64 = 256 * 1024 * 1024;

/// Extracted HEIF image data ready for HEVC decoding.
pub struct HeifImage {
    pub hvcc: Vec<u8>,
    /// For single-tile: one bitstream. For grid: one bitstream per tile.
    pub tiles: Vec<Vec<u8>>,
    /// Grid layout: (cols, rows). (1,1) for single-tile images.
    pub grid_cols: u32,
    pub grid_rows: u32,
    pub width: u32,
    pub height: u32,
}

/// Parse an ISO 23008-12 ImageGrid descriptor.
/// Returns (rows, cols, output_width, output_height).
fn parse_grid_descriptor(data: &[u8]) -> Result<(u32, u32, u32, u32)> {
    ensure!(data.len() >= 8, "grid descriptor too short ({} bytes)", data.len());
    let _version = data[0];
    let flags = data[1];
    let rows = data[2] as u32 + 1;
    let cols = data[3] as u32 + 1;
    let (width, height) = if flags & 1 != 0 {
        ensure!(data.len() >= 12, "grid descriptor too short for 32-bit dims ({} bytes)", data.len());
        (
            u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
            u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        )
    } else {
        (
            u16::from_be_bytes([data[4], data[5]]) as u32,
            u16::from_be_bytes([data[6], data[7]]) as u32,
        )
    };
    Ok((rows, cols, width, height))
}

/// Parse a HEIF/HEIC container and extract the primary image's HEVC data.
///
/// Handles both single-tile HEVC items and image grids (iPhone-style).
pub fn parse_heif<R: Read + Seek>(reader: &mut R) -> Result<HeifImage> {
    let file_end = reader.seek(SeekFrom::End(0))?;

    let (meta_start, meta_end) = isobmff_find_box(reader, 0, file_end, b"meta")?
        .context("No meta box found in HEIF container")?;

    // meta is a FullBox: skip version(1) + flags(3)
    let children_start = meta_start + 4;

    let primary_item_id = parse_pitm(reader, children_start, meta_end)?;

    let (iinf_start, _) = isobmff_find_box(reader, children_start, meta_end, b"iinf")?
        .context("No iinf box found")?;

    let item_type = find_item_type_by_id(reader, iinf_start, primary_item_id)?;

    let (iloc_start, _) = isobmff_find_box(reader, children_start, meta_end, b"iloc")?
        .context("No iloc box found")?;

    let (iprp_start, iprp_end) = isobmff_find_box(reader, children_start, meta_end, b"iprp")?
        .context("No iprp box found")?;

    let (ipco_start, ipco_end) = isobmff_find_box(reader, iprp_start, iprp_end, b"ipco")?
        .context("No ipco box found")?;

    let hvcc = extract_hvcc_from_ipco(reader, ipco_start, ipco_end)?;

    if &item_type == b"hvc1" || &item_type == b"hev1" {
        // Single-tile HEVC: primary item IS the image
        let (width, height) = extract_ispe_from_ipco(reader, ipco_start, ipco_end)?;
        let (offset, length) = isobmff_find_item_extent(reader, iloc_start, primary_item_id)?
            .context(format!(
                "Primary item (id={}) extent not found in iloc",
                primary_item_id,
            ))?;

        if length > MAX_TILE_DATA_SIZE {
            bail!(
                "Primary item (id={}) data size {} bytes exceeds limit of {} bytes",
                primary_item_id, length, MAX_TILE_DATA_SIZE,
            );
        }

        reader.seek(SeekFrom::Start(offset))?;
        let mut bitstream = vec![0u8; length as usize];
        reader.read_exact(&mut bitstream)?;

        Ok(HeifImage { hvcc, tiles: vec![bitstream], grid_cols: 1, grid_rows: 1, width, height })
    } else if &item_type == b"grid" {
        // Image grid: find the largest ispe for output dimensions
        let (width, height) = extract_largest_ispe(reader, ipco_start, ipco_end)?;
        parse_grid_image(reader, children_start, meta_end, iloc_start,
                         primary_item_id, &hvcc, width, height)
    } else {
        bail!(
            "Primary item (id={}) has type '{}', expected hvc1/hev1/grid",
            primary_item_id,
            String::from_utf8_lossy(&item_type)
        );
    }
}

/// Parse an image grid: read the grid descriptor from iloc, find tile items, read their bitstreams.
fn parse_grid_image<R: Read + Seek>(
    reader: &mut R,
    children_start: u64,
    meta_end: u64,
    iloc_start: u64,
    grid_item_id: u32,
    hvcc: &[u8],
    ispe_width: u32,
    ispe_height: u32,
) -> Result<HeifImage> {
    let tile_ids = find_derived_image_refs(reader, children_start, meta_end, grid_item_id)?;

    if tile_ids.len() > MAX_TILES {
        bail!(
            "Grid item (id={}) references {} tiles, exceeds security limit of {}",
            grid_item_id, tile_ids.len(), MAX_TILES,
        );
    }

    // The grid item's iloc data contains the grid descriptor, not tile bitstreams.
    let (rows, cols, width, height) = match isobmff_find_item_extent(reader, iloc_start, grid_item_id)? {
        Some((offset, length)) if length >= 8 && length <= 64 => {
            reader.seek(SeekFrom::Start(offset))?;
            let mut desc = vec![0u8; length as usize];
            reader.read_exact(&mut desc)?;
            match parse_grid_descriptor(&desc) {
                Ok((r, c, w, h)) => {
                    if c * r == tile_ids.len() as u32 {
                        (r, c, w, h)
                    } else {
                        // Grid descriptor tile count doesn't match iref dimg count; fall back
                        let (c, r) = infer_grid_layout(tile_ids.len() as u32, ispe_width, ispe_height);
                        (r, c, ispe_width, ispe_height)
                    }
                }
                Err(_) => {
                    let (c, r) = infer_grid_layout(tile_ids.len() as u32, ispe_width, ispe_height);
                    (r, c, ispe_width, ispe_height)
                }
            }
        }
        _ => {
            // No iloc data for grid item or too small/large; fall back to inference
            let (c, r) = infer_grid_layout(tile_ids.len() as u32, ispe_width, ispe_height);
            (r, c, ispe_width, ispe_height)
        }
    };

    // Read each tile's bitstream separately
    let mut tiles = Vec::with_capacity(tile_ids.len());
    for (i, &tile_id) in tile_ids.iter().enumerate() {
        let (offset, length) = isobmff_find_item_extent(reader, iloc_start, tile_id)?
            .context(format!("Tile {} (item id={}) extent not found in iloc", i, tile_id))?;

        if length > MAX_TILE_DATA_SIZE {
            bail!(
                "Tile {} (item id={}) data size {} bytes exceeds limit of {} bytes",
                i, tile_id, length, MAX_TILE_DATA_SIZE,
            );
        }

        reader.seek(SeekFrom::Start(offset))?;
        let mut tile_data = vec![0u8; length as usize];
        reader.read_exact(&mut tile_data)?;
        tiles.push(tile_data);
    }

    Ok(HeifImage {
        hvcc: hvcc.to_vec(),
        tiles,
        grid_cols: cols,
        grid_rows: rows,
        width,
        height,
    })
}

/// Find item references of type 'dimg' (derived image) from the 'iref' box.
/// Try to determine grid layout from tile count and image dimensions.
fn infer_grid_layout(tile_count: u32, width: u32, height: u32) -> (u32, u32) {
    // Common tile sizes: 512x512, 256x256
    for tile_size in [512u32, 256, 1024, 384, 640] {
        let c = (width + tile_size - 1) / tile_size;
        let r = (height + tile_size - 1) / tile_size;
        if c * r == tile_count {
            return (c, r);
        }
    }
    // Fallback: single row
    (tile_count, 1)
}

fn find_derived_image_refs<R: Read + Seek>(
    reader: &mut R,
    children_start: u64,
    meta_end: u64,
    from_item_id: u32,
) -> Result<Vec<u32>> {
    let (iref_start, iref_end) = match isobmff_find_box(reader, children_start, meta_end, b"iref")? {
        Some(v) => v,
        None => bail!("No iref box found (required for grid images)"),
    };

    // iref is a FullBox: version(1) + flags(3)
    reader.seek(SeekFrom::Start(iref_start))?;
    let mut vf = [0u8; 4];
    reader.read_exact(&mut vf)?;
    let version = vf[0];

    let mut pos = reader.stream_position()?;

    while pos + 8 < iref_end {
        reader.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        if reader.read(&mut hdr)? < 8 {
            break;
        }
        let box_size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        let ref_type = &hdr[4..8];
        let box_end = pos + box_size;

        if ref_type == b"dimg" {
            // Read from_item_ID
            let from_id = if version == 0 {
                let mut buf = [0u8; 2];
                reader.read_exact(&mut buf)?;
                u16::from_be_bytes(buf) as u32
            } else {
                let mut buf = [0u8; 4];
                reader.read_exact(&mut buf)?;
                u32::from_be_bytes(buf)
            };

            let mut count_buf = [0u8; 2];
            reader.read_exact(&mut count_buf)?;
            let ref_count = u16::from_be_bytes(count_buf) as usize;

            if ref_count > MAX_TILES {
                bail!(
                    "dimg reference at offset {:#x}: ref count {} exceeds security limit of {}",
                    pos, ref_count, MAX_TILES,
                );
            }

            if from_id == from_item_id {
                let mut ids = Vec::with_capacity(ref_count);
                for _ in 0..ref_count {
                    let id = if version == 0 {
                        let mut buf = [0u8; 2];
                        reader.read_exact(&mut buf)?;
                        u16::from_be_bytes(buf) as u32
                    } else {
                        let mut buf = [0u8; 4];
                        reader.read_exact(&mut buf)?;
                        u32::from_be_bytes(buf)
                    };
                    ids.push(id);
                }
                return Ok(ids);
            }
        }

        if box_end <= pos {
            break;
        }
        pos = box_end;
    }

    bail!(
        "No dimg references found for grid item id={} in iref at offset {:#x}",
        from_item_id, iref_start,
    );
}

fn parse_pitm<R: Read + Seek>(
    reader: &mut R,
    children_start: u64,
    meta_end: u64,
) -> Result<u32> {
    let (pitm_start, _) = isobmff_find_box(reader, children_start, meta_end, b"pitm")?
        .context("No pitm box found")?;

    reader.seek(SeekFrom::Start(pitm_start))?;
    let mut vf = [0u8; 4];
    reader.read_exact(&mut vf)?;
    let version = vf[0];

    if version == 0 {
        let mut buf = [0u8; 2];
        reader.read_exact(&mut buf)?;
        Ok(u16::from_be_bytes(buf) as u32)
    } else {
        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf)?;
        Ok(u32::from_be_bytes(buf))
    }
}

fn find_item_type_by_id<R: Read + Seek>(
    reader: &mut R,
    iinf_start: u64,
    target_id: u32,
) -> Result<[u8; 4]> {
    reader.seek(SeekFrom::Start(iinf_start))?;
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

    for _ in 0..entry_count {
        let box_pos = reader.stream_position()?;
        let mut hdr = [0u8; 8];
        if reader.read(&mut hdr)? < 8 { break; }
        let size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        let box_end = box_pos + size;

        if &hdr[4..8] == b"infe" {
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
                reader.seek(SeekFrom::Current(2))?; // item_protection_index
                let mut item_type = [0u8; 4];
                reader.read_exact(&mut item_type)?;

                if item_id == target_id {
                    return Ok(item_type);
                }
            }
        }

        if box_end <= box_pos { break; }
        reader.seek(SeekFrom::Start(box_end))?;
    }
    bail!(
        "Item id={} not found in iinf (scanned {} entries starting at offset {:#x})",
        target_id, entry_count, iinf_start,
    );
}

fn extract_hvcc_from_ipco<R: Read + Seek>(
    reader: &mut R,
    ipco_start: u64,
    ipco_end: u64,
) -> Result<Vec<u8>> {
    let (hvcc_start, hvcc_end) = isobmff_find_box(reader, ipco_start, ipco_end, b"hvcC")?
        .context("No hvcC property found in ipco")?;

    let len = (hvcc_end - hvcc_start) as usize;
    if len > 100_000 {
        bail!(
            "hvcC box at offset {:#x}: size {} bytes exceeds 100 KB safety limit",
            hvcc_start, len,
        );
    }

    reader.seek(SeekFrom::Start(hvcc_start))?;
    let mut data = vec![0u8; len];
    reader.read_exact(&mut data)?;
    Ok(data)
}

/// Find the largest ispe dimensions in ipco (for grid items, this is the output dimensions).
fn extract_largest_ispe<R: Read + Seek>(
    reader: &mut R,
    ipco_start: u64,
    ipco_end: u64,
) -> Result<(u32, u32)> {
    let mut best = (0u32, 0u32);
    let mut pos = ipco_start;
    while pos + 8 < ipco_end {
        reader.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        if reader.read(&mut hdr)? < 8 { break; }
        let size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        if &hdr[4..8] == b"ispe" {
            // FullBox: version(1) + flags(3) + width(4) + height(4)
            reader.seek(SeekFrom::Current(4))?;
            let mut dims = [0u8; 8];
            reader.read_exact(&mut dims)?;
            let w = u32::from_be_bytes([dims[0], dims[1], dims[2], dims[3]]);
            let h = u32::from_be_bytes([dims[4], dims[5], dims[6], dims[7]]);
            if (w as u64 * h as u64) > (best.0 as u64 * best.1 as u64) {
                best = (w, h);
            }
        }
        if size == 0 || pos + size <= pos { break; }
        pos += size;
    }
    if best.0 == 0 || best.1 == 0 {
        bail!(
            "No valid ispe found in ipco (searched range {:#x}..{:#x})",
            ipco_start, ipco_end,
        );
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::parse_grid_descriptor;

    #[test]
    fn grid_descriptor_16bit_dims() {
        // 6 cols (5+1), 8 rows (7+1), 4032x3024, flags=0 (16-bit dims)
        let data = [0u8, 0, 7, 5, 0x0F, 0xC0, 0x0B, 0xD0];
        let (rows, cols, w, h) = parse_grid_descriptor(&data).unwrap();
        assert_eq!((rows, cols, w, h), (8, 6, 4032, 3024));
    }

    #[test]
    fn grid_descriptor_32bit_dims() {
        // 2 cols (1+1), 3 rows (2+1), 70000x50000, flags=1 (32-bit dims)
        let mut data = [0u8; 12];
        data[1] = 1; // flags: 32-bit
        data[2] = 2; // rows - 1
        data[3] = 1; // cols - 1
        data[4..8].copy_from_slice(&70000u32.to_be_bytes());
        data[8..12].copy_from_slice(&50000u32.to_be_bytes());
        let (rows, cols, w, h) = parse_grid_descriptor(&data).unwrap();
        assert_eq!((rows, cols, w, h), (3, 2, 70000, 50000));
    }

    #[test]
    fn grid_descriptor_too_short() {
        assert!(parse_grid_descriptor(&[0; 7]).is_err());
    }

    #[test]
    fn grid_descriptor_32bit_too_short() {
        let mut data = [0u8; 8];
        data[1] = 1; // flags: 32-bit, but only 8 bytes
        assert!(parse_grid_descriptor(&data).is_err());
    }
}

fn extract_ispe_from_ipco<R: Read + Seek>(
    reader: &mut R,
    ipco_start: u64,
    ipco_end: u64,
) -> Result<(u32, u32)> {
    let (ispe_start, _) = isobmff_find_box(reader, ipco_start, ipco_end, b"ispe")?
        .context("No ispe property found in ipco")?;

    reader.seek(SeekFrom::Start(ispe_start + 4))?;
    let mut dims = [0u8; 8];
    reader.read_exact(&mut dims)?;

    let width = u32::from_be_bytes([dims[0], dims[1], dims[2], dims[3]]);
    let height = u32::from_be_bytes([dims[4], dims[5], dims[6], dims[7]]);

    if width == 0 || height == 0 {
        bail!("Invalid ispe dimensions: {}x{}", width, height);
    }

    Ok((width, height))
}
