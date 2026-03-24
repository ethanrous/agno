use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

use anyhow::{Context, Result, bail, ensure};

use super::isobmff::{isobmff_find_box, isobmff_read_item_data};

/// Maximum number of tiles in a grid image.
const MAX_TILES: usize = 16_384;

/// Maximum byte size for a single tile bitstream (256 MB).
const MAX_TILE_DATA_SIZE: u64 = 256 * 1024 * 1024;

/// Maximum number of ipma entries (items with property associations).
const MAX_IPMA_ENTRIES: u32 = 10_000;

/// Maximum number of property associations per item.
const MAX_ASSOCIATIONS_PER_ITEM: u8 = 128;

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
    ensure!(
        data.len() >= 8,
        "grid descriptor too short ({} bytes)",
        data.len()
    );
    let _version = data[0];
    let flags = data[1];
    let rows = data[2] as u32 + 1;
    let cols = data[3] as u32 + 1;
    let (width, height) = if flags & 1 != 0 {
        ensure!(
            data.len() >= 12,
            "grid descriptor too short for 32-bit dims ({} bytes)",
            data.len()
        );
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

/// Parse an ipma (Item Property Association) box.
///
/// Returns a map from item_id to a list of 1-based property indices into ipco.
/// Ported from libheif Box_ipma::parse (box.cc).
fn parse_ipma<R: Read + Seek>(
    reader: &mut R,
    ipma_start: u64,
    ipma_end: u64,
) -> Result<HashMap<u32, Vec<u16>>> {
    reader.seek(SeekFrom::Start(ipma_start))?;

    // FullBox: version(1) + flags(3)
    let mut vf = [0u8; 4];
    reader.read_exact(&mut vf)?;
    let version = vf[0];
    let flags = u32::from_be_bytes([0, vf[1], vf[2], vf[3]]);

    ensure!(version <= 1, "Unsupported ipma version {}", version);

    let mut buf4 = [0u8; 4];
    reader.read_exact(&mut buf4)?;
    let entry_count = u32::from_be_bytes(buf4);

    ensure!(
        entry_count <= MAX_IPMA_ENTRIES,
        "ipma entry count {} exceeds limit {}",
        entry_count,
        MAX_IPMA_ENTRIES,
    );

    let mut result: HashMap<u32, Vec<u16>> = HashMap::new();

    for _ in 0..entry_count {
        if reader.stream_position()? >= ipma_end {
            break;
        }

        let item_id = if version < 1 {
            let mut buf = [0u8; 2];
            reader.read_exact(&mut buf)?;
            u16::from_be_bytes(buf) as u32
        } else {
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            u32::from_be_bytes(buf)
        };

        let mut assoc_count_buf = [0u8; 1];
        reader.read_exact(&mut assoc_count_buf)?;
        let assoc_count = assoc_count_buf[0];

        ensure!(
            assoc_count <= MAX_ASSOCIATIONS_PER_ITEM,
            "ipma item {} has {} associations, exceeds limit {}",
            item_id,
            assoc_count,
            MAX_ASSOCIATIONS_PER_ITEM,
        );

        let mut indices = Vec::with_capacity(assoc_count as usize);
        for _ in 0..assoc_count {
            let property_index = if flags & 1 != 0 {
                // 16-bit: essential(1) | property_index(15)
                let mut buf = [0u8; 2];
                reader.read_exact(&mut buf)?;
                u16::from_be_bytes(buf) & 0x7FFF
            } else {
                // 8-bit: essential(1) | property_index(7)
                let mut buf = [0u8; 1];
                reader.read_exact(&mut buf)?;
                (buf[0] & 0x7F) as u16
            };
            indices.push(property_index);
        }

        result.insert(item_id, indices);
    }

    Ok(result)
}

/// Get the content range of the Nth child box in ipco (1-based index).
///
/// Returns (content_start, box_end) of the child box at the given index,
/// or None if the index is out of range.
fn get_ipco_property_by_index<R: Read + Seek>(
    reader: &mut R,
    ipco_start: u64,
    ipco_end: u64,
    property_index: u16,
) -> Result<Option<([u8; 4], u64, u64)>> {
    ensure!(property_index >= 1, "ipco property index must be >= 1");

    let mut pos = ipco_start;
    let mut current_index: u16 = 0;

    while pos + 8 <= ipco_end {
        reader.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        if reader.read(&mut hdr)? < 8 {
            break;
        }
        let size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        let box_type: [u8; 4] = [hdr[4], hdr[5], hdr[6], hdr[7]];

        let (content_start, box_end) = if size == 1 {
            let mut ext = [0u8; 8];
            reader.read_exact(&mut ext)?;
            let large_size = u64::from_be_bytes(ext);
            ensure!(
                large_size >= 16,
                "ipco child box extended size {} too small",
                large_size
            );
            (pos + 16, pos + large_size)
        } else if size == 0 {
            (pos + 8, ipco_end)
        } else {
            ensure!(size >= 8, "ipco child box size {} too small", size);
            (pos + 8, pos + size as u64)
        };

        ensure!(
            box_end <= ipco_end,
            "ipco child box at {:#x} extends past ipco boundary",
            pos,
        );

        current_index += 1;
        if current_index == property_index {
            return Ok(Some((box_type, content_start, box_end)));
        }

        if box_end <= pos {
            break;
        }
        pos = box_end;
    }

    Ok(None)
}

/// Find the hvcC property associated with a specific item via ipma.
///
/// Iterates the item's property indices (from ipma) and returns the content
/// of the first hvcC box found among them.
fn extract_item_hvcc<R: Read + Seek>(
    reader: &mut R,
    ipco_start: u64,
    ipco_end: u64,
    property_indices: &[u16],
) -> Result<Vec<u8>> {
    for &idx in property_indices {
        if idx == 0 {
            continue;
        }
        if let Some((box_type, content_start, box_end)) =
            get_ipco_property_by_index(reader, ipco_start, ipco_end, idx)?
        {
            if &box_type == b"hvcC" {
                let len = (box_end - content_start) as usize;
                ensure!(
                    len <= 100_000,
                    "hvcC box at offset {:#x}: size {} bytes exceeds 100 KB safety limit",
                    content_start,
                    len,
                );
                reader.seek(SeekFrom::Start(content_start))?;
                let mut data = vec![0u8; len];
                reader.read_exact(&mut data)?;
                return Ok(data);
            }
        }
    }
    bail!(
        "No hvcC property found among item's {} property associations",
        property_indices.len()
    )
}

/// Find the ispe (image spatial extents) property associated with a specific item via ipma.
fn extract_item_ispe<R: Read + Seek>(
    reader: &mut R,
    ipco_start: u64,
    ipco_end: u64,
    property_indices: &[u16],
) -> Result<(u32, u32)> {
    for &idx in property_indices {
        if idx == 0 {
            continue;
        }
        if let Some((box_type, content_start, _)) =
            get_ipco_property_by_index(reader, ipco_start, ipco_end, idx)?
        {
            if &box_type == b"ispe" {
                // ispe is a FullBox: version(1) + flags(3) + width(4) + height(4)
                reader.seek(SeekFrom::Start(content_start + 4))?;
                let mut dims = [0u8; 8];
                reader.read_exact(&mut dims)?;
                let w = u32::from_be_bytes([dims[0], dims[1], dims[2], dims[3]]);
                let h = u32::from_be_bytes([dims[4], dims[5], dims[6], dims[7]]);
                if w > 0 && h > 0 {
                    return Ok((w, h));
                }
            }
        }
    }
    bail!(
        "No valid ispe property found among item's {} property associations",
        property_indices.len()
    )
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

    // idat box: optional, needed when iloc construction_method=1
    let idat_content_start = isobmff_find_box(reader, children_start, meta_end, b"idat")?
        .map(|(content_start, _)| content_start);

    let (iprp_start, iprp_end) = isobmff_find_box(reader, children_start, meta_end, b"iprp")?
        .context("No iprp box found")?;

    let (ipco_start, ipco_end) =
        isobmff_find_box(reader, iprp_start, iprp_end, b"ipco")?.context("No ipco box found")?;

    // Parse ipma to get per-item property associations
    let (ipma_start, ipma_end) = isobmff_find_box(reader, iprp_start, iprp_end, b"ipma")?
        .context("No ipma box found in iprp")?;
    let ipma = parse_ipma(reader, ipma_start, ipma_end)?;

    if &item_type == b"hvc1" || &item_type == b"hev1" {
        // Single-tile HEVC: primary item IS the image
        let primary_props = ipma.get(&primary_item_id).context(format!(
            "Primary item (id={}) has no ipma associations",
            primary_item_id
        ))?;

        let hvcc =
            extract_item_hvcc(reader, ipco_start, ipco_end, primary_props).context(format!(
                "Failed to find hvcC for primary item (id={})",
                primary_item_id
            ))?;
        let (width, height) = extract_item_ispe(reader, ipco_start, ipco_end, primary_props)
            .context(format!(
                "Failed to find ispe for primary item (id={})",
                primary_item_id
            ))?;

        let bitstream =
            isobmff_read_item_data(reader, iloc_start, primary_item_id, idat_content_start)?
                .context(format!(
                    "Primary item (id={}) not found in iloc",
                    primary_item_id,
                ))?;

        if bitstream.len() as u64 > MAX_TILE_DATA_SIZE {
            bail!(
                "Primary item (id={}) data size {} bytes exceeds limit of {} bytes",
                primary_item_id,
                bitstream.len(),
                MAX_TILE_DATA_SIZE,
            );
        }

        Ok(HeifImage {
            hvcc,
            tiles: vec![bitstream],
            grid_cols: 1,
            grid_rows: 1,
            width,
            height,
        })
    } else if &item_type == b"grid" {
        // Image grid: get output dimensions from the grid item's ispe
        let grid_props = ipma.get(&primary_item_id).context(format!(
            "Grid item (id={}) has no ipma associations",
            primary_item_id
        ))?;
        let (ispe_width, ispe_height) = extract_item_ispe(reader, ipco_start, ipco_end, grid_props)
            .context(format!(
                "Failed to find ispe for grid item (id={})",
                primary_item_id
            ))?;

        parse_grid_image(
            reader,
            children_start,
            meta_end,
            iloc_start,
            idat_content_start,
            primary_item_id,
            ipco_start,
            ipco_end,
            &ipma,
            ispe_width,
            ispe_height,
        )
    } else {
        bail!(
            "Primary item (id={}) has type '{}', expected hvc1/hev1/grid",
            primary_item_id,
            String::from_utf8_lossy(&item_type)
        );
    }
}

/// Parse an image grid: read the grid descriptor from iloc, find tile items, read their bitstreams.
///
/// Uses ipma to look up the hvcC for the first tile item, ensuring we get the
/// correct decoder configuration even when multiple hvcC boxes exist in ipco.
fn parse_grid_image<R: Read + Seek>(
    reader: &mut R,
    children_start: u64,
    meta_end: u64,
    iloc_start: u64,
    idat_content_start: Option<u64>,
    grid_item_id: u32,
    ipco_start: u64,
    ipco_end: u64,
    ipma: &HashMap<u32, Vec<u16>>,
    ispe_width: u32,
    ispe_height: u32,
) -> Result<HeifImage> {
    let tile_ids = find_derived_image_refs(reader, children_start, meta_end, grid_item_id)?;

    if tile_ids.len() > MAX_TILES {
        bail!(
            "Grid item (id={}) references {} tiles, exceeds security limit of {}",
            grid_item_id,
            tile_ids.len(),
            MAX_TILES,
        );
    }

    // Look up hvcC from the first tile item's properties
    let first_tile_id = *tile_ids.first().context("Grid has no tile references")?;
    let tile_props = ipma.get(&first_tile_id).context(format!(
        "Tile item (id={}) has no ipma associations",
        first_tile_id
    ))?;
    let hvcc = extract_item_hvcc(reader, ipco_start, ipco_end, tile_props).context(format!(
        "Failed to find hvcC for tile item (id={})",
        first_tile_id
    ))?;

    // The grid item's iloc data contains the grid descriptor, not tile bitstreams.
    let (rows, cols, width, height) =
        match isobmff_read_item_data(reader, iloc_start, grid_item_id, idat_content_start)? {
            Some(desc) if desc.len() >= 8 && desc.len() <= 64 => {
                match parse_grid_descriptor(&desc) {
                    Ok((r, c, w, h)) => {
                        if c * r == tile_ids.len() as u32 {
                            (r, c, w, h)
                        } else {
                            let (c, r) =
                                infer_grid_layout(tile_ids.len() as u32, ispe_width, ispe_height);
                            (r, c, ispe_width, ispe_height)
                        }
                    }
                    Err(_) => {
                        let (c, r) =
                            infer_grid_layout(tile_ids.len() as u32, ispe_width, ispe_height);
                        (r, c, ispe_width, ispe_height)
                    }
                }
            }
            _ => {
                let (c, r) = infer_grid_layout(tile_ids.len() as u32, ispe_width, ispe_height);
                (r, c, ispe_width, ispe_height)
            }
        };

    // Read each tile's bitstream (concatenating all extents per tile)
    let mut tiles = Vec::with_capacity(tile_ids.len());
    for (i, &tile_id) in tile_ids.iter().enumerate() {
        let tile_data =
            isobmff_read_item_data(reader, iloc_start, tile_id, idat_content_start)?.context(
                format!("Tile {} (item id={}) not found in iloc", i, tile_id),
            )?;

        if tile_data.len() as u64 > MAX_TILE_DATA_SIZE {
            bail!(
                "Tile {} (item id={}) data size {} bytes exceeds limit of {} bytes",
                i,
                tile_id,
                tile_data.len(),
                MAX_TILE_DATA_SIZE,
            );
        }

        tiles.push(tile_data);
    }

    Ok(HeifImage {
        hvcc,
        tiles,
        grid_cols: cols,
        grid_rows: rows,
        width,
        height,
    })
}

/// Try to determine grid layout from tile count and image dimensions.
fn infer_grid_layout(tile_count: u32, width: u32, height: u32) -> (u32, u32) {
    // Square tile sizes
    for tile_size in [512u32, 256, 1024, 384, 640] {
        let c = (width + tile_size - 1) / tile_size;
        let r = (height + tile_size - 1) / tile_size;
        if c * r == tile_count {
            return (c, r);
        }
    }
    // Non-square tile sizes used by iPhones
    for (tw, th) in [
        (640u32, 896u32),
        (896, 640),
        (480, 640),
        (640, 480),
        (960, 640),
        (640, 960),
    ] {
        let c = (width + tw - 1) / tw;
        let r = (height + th - 1) / th;
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
    let (iref_start, iref_end) = match isobmff_find_box(reader, children_start, meta_end, b"iref")?
    {
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
                    pos,
                    ref_count,
                    MAX_TILES,
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
        from_item_id,
        iref_start,
    );
}

fn parse_pitm<R: Read + Seek>(reader: &mut R, children_start: u64, meta_end: u64) -> Result<u32> {
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
        if reader.read(&mut hdr)? < 8 {
            break;
        }
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

        if box_end <= box_pos {
            break;
        }
        reader.seek(SeekFrom::Start(box_end))?;
    }
    bail!(
        "Item id={} not found in iinf (scanned {} entries starting at offset {:#x})",
        target_id,
        entry_count,
        iinf_start,
    );
}

#[cfg(test)]
mod tests {
    use super::{
        extract_item_hvcc, extract_item_ispe, get_ipco_property_by_index, parse_grid_descriptor,
        parse_ipma,
    };
    use std::io::Cursor;

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

    /// Build a synthetic ipma box body (version 0, 8-bit property indices).
    /// Two items: item 1 -> [1, 3], item 2 -> [2]
    fn make_ipma_v0_8bit() -> Vec<u8> {
        let mut buf = Vec::new();
        // FullBox: version=0, flags=0 (8-bit indices)
        buf.extend_from_slice(&[0, 0, 0, 0]);
        // entry_count = 2
        buf.extend_from_slice(&2u32.to_be_bytes());
        // Entry 1: item_id=1 (2 bytes), assoc_count=2
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.push(2); // assoc_count
        buf.push(0x01); // essential=0, index=1
        buf.push(0x83); // essential=1, index=3
        // Entry 2: item_id=5 (2 bytes), assoc_count=1
        buf.extend_from_slice(&5u16.to_be_bytes());
        buf.push(1);
        buf.push(0x02); // essential=0, index=2
        buf
    }

    #[test]
    fn ipma_v0_8bit_parses_correctly() {
        let data = make_ipma_v0_8bit();
        let mut cursor = Cursor::new(&data);
        let ipma = parse_ipma(&mut cursor, 0, data.len() as u64).unwrap();
        assert_eq!(ipma.get(&1).unwrap(), &vec![1u16, 3]);
        assert_eq!(ipma.get(&5).unwrap(), &vec![2u16]);
        assert!(ipma.get(&99).is_none());
    }

    /// Build a synthetic ipma box body (version 1, 16-bit property indices).
    /// One item: item 42 -> [15, 16]
    fn make_ipma_v1_16bit() -> Vec<u8> {
        let mut buf = Vec::new();
        // FullBox: version=1, flags=1 (16-bit indices)
        buf.extend_from_slice(&[1, 0, 0, 1]);
        // entry_count = 1
        buf.extend_from_slice(&1u32.to_be_bytes());
        // Entry: item_id=42 (4 bytes for version >= 1), assoc_count=2
        buf.extend_from_slice(&42u32.to_be_bytes());
        buf.push(2);
        // essential=1, index=15 -> 0x800F
        buf.extend_from_slice(&0x800Fu16.to_be_bytes());
        // essential=0, index=16 -> 0x0010
        buf.extend_from_slice(&0x0010u16.to_be_bytes());
        buf
    }

    #[test]
    fn ipma_v1_16bit_parses_correctly() {
        let data = make_ipma_v1_16bit();
        let mut cursor = Cursor::new(&data);
        let ipma = parse_ipma(&mut cursor, 0, data.len() as u64).unwrap();
        assert_eq!(ipma.get(&42).unwrap(), &vec![15u16, 16]);
    }

    /// Build a synthetic ipco with 3 child boxes:
    /// index 1: ispe (20 bytes: 8 hdr + 4 fullbox + 8 dims)
    /// index 2: hvcC (16 bytes: 8 hdr + 8 content)
    /// index 3: ispe (20 bytes, different dims)
    fn make_ipco() -> Vec<u8> {
        let mut buf = Vec::new();

        // Box 1: ispe, 640x480
        let ispe_size: u32 = 20;
        buf.extend_from_slice(&ispe_size.to_be_bytes());
        buf.extend_from_slice(b"ispe");
        buf.extend_from_slice(&[0, 0, 0, 0]); // version + flags
        buf.extend_from_slice(&640u32.to_be_bytes());
        buf.extend_from_slice(&480u32.to_be_bytes());

        // Box 2: hvcC, some fake content
        let hvcc_size: u32 = 16;
        buf.extend_from_slice(&hvcc_size.to_be_bytes());
        buf.extend_from_slice(b"hvcC");
        buf.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE]);

        // Box 3: ispe, 1920x1080
        let ispe2_size: u32 = 20;
        buf.extend_from_slice(&ispe2_size.to_be_bytes());
        buf.extend_from_slice(b"ispe");
        buf.extend_from_slice(&[0, 0, 0, 0]);
        buf.extend_from_slice(&1920u32.to_be_bytes());
        buf.extend_from_slice(&1080u32.to_be_bytes());

        buf
    }

    #[test]
    fn ipco_property_by_index() {
        let data = make_ipco();
        let mut cursor = Cursor::new(&data);
        let end = data.len() as u64;

        let (box_type, _, _) = get_ipco_property_by_index(&mut cursor, 0, end, 1)
            .unwrap()
            .unwrap();
        assert_eq!(&box_type, b"ispe");

        let (box_type, _, _) = get_ipco_property_by_index(&mut cursor, 0, end, 2)
            .unwrap()
            .unwrap();
        assert_eq!(&box_type, b"hvcC");

        let (box_type, _, _) = get_ipco_property_by_index(&mut cursor, 0, end, 3)
            .unwrap()
            .unwrap();
        assert_eq!(&box_type, b"ispe");

        assert!(
            get_ipco_property_by_index(&mut cursor, 0, end, 4)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn extract_hvcc_via_ipma() {
        let data = make_ipco();
        let mut cursor = Cursor::new(&data);
        let end = data.len() as u64;

        // Item with properties [1, 2] should find hvcC at index 2
        let hvcc = extract_item_hvcc(&mut cursor, 0, end, &[1, 2]).unwrap();
        assert_eq!(hvcc, &[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE]);

        // Item with properties [1, 3] (two ispe, no hvcC) should fail
        assert!(extract_item_hvcc(&mut cursor, 0, end, &[1, 3]).is_err());
    }

    #[test]
    fn extract_ispe_via_ipma() {
        let data = make_ipco();
        let mut cursor = Cursor::new(&data);
        let end = data.len() as u64;

        // Item with properties [1, 2]: first ispe is at index 1 -> 640x480
        let (w, h) = extract_item_ispe(&mut cursor, 0, end, &[1, 2]).unwrap();
        assert_eq!((w, h), (640, 480));

        // Item with properties [3, 2]: first ispe is at index 3 -> 1920x1080
        let (w, h) = extract_item_ispe(&mut cursor, 0, end, &[3, 2]).unwrap();
        assert_eq!((w, h), (1920, 1080));

        // Item with only hvcC (index 2): no ispe -> error
        assert!(extract_item_ispe(&mut cursor, 0, end, &[2]).is_err());
    }
}
