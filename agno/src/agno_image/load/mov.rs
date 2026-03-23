use std::{
    error::Error,
    fs::File,
    io::{Read, Seek, SeekFrom},
};

use crate::{
    agno_image::AgnoImage,
    codec::hevc::decode_hevc_still,
    codec::isobmff::{isobmff_find_box, isobmff_find_item_id_by_type, isobmff_read_item_data},
    exif::ExifContext,
};

/// Extract a thumbnail from a MOV/MP4 ISOBMFF container.
///
/// Tries four strategies in order:
/// 1. HEIF-style item references (iPhone MOV with top-level `meta` box)
/// 2. Thumbnail track (`pict` or small `vide` handler in `moov/trak`)
/// 3. JPEG scan in `moov/udta`
/// 4. Decode first HEVC keyframe natively
///
/// Returns an error if no strategy succeeds.
pub fn load_mov_thumbnail(file: &mut File, exif: ExifContext) -> Result<AgnoImage, Box<dyn Error>> {
    if let Some(data) = try_heif_style_thumbnail(file)? {
        return decode_jpeg_thumbnail(&data, exif);
    }

    if let Some(data) = try_thumbnail_track(file)? {
        return decode_jpeg_thumbnail(&data, exif);
    }

    if let Some(data) = try_udta_jpeg(file)? {
        return decode_jpeg_thumbnail(&data, exif);
    }

    if let Ok(img) = try_hevc_keyframe(file, exif.clone()) {
        return Ok(img);
    }

    Err("No embedded thumbnail found in video file".into())
}

/// Strategy A: HEIF-style item references.
/// iPhone MOV files have a top-level `meta` box with `iinf`/`iloc` containing
/// a JPEG item (type `jpeg` or `JPEG`).
fn try_heif_style_thumbnail(file: &mut File) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    let file_end = file.seek(SeekFrom::End(0))?;

    let (meta_start, meta_end) = match isobmff_find_box(file, 0, file_end, b"meta")? {
        Some(v) => v,
        None => return Ok(None),
    };

    // FullBox: skip version(1) + flags(3)
    let children_start = meta_start + 4;

    let iinf_start = match isobmff_find_box(file, children_start, meta_end, b"iinf")? {
        Some((s, _)) => s,
        None => return Ok(None),
    };
    let iloc_start = match isobmff_find_box(file, children_start, meta_end, b"iloc")? {
        Some((s, _)) => s,
        None => return Ok(None),
    };

    let idat_content_start = isobmff_find_box(file, children_start, meta_end, b"idat")?
        .map(|(cs, _)| cs);

    // Try both "jpeg" and "JPEG" item types
    for target in [b"jpeg", b"JPEG"] {
        if let Some(item_id) = isobmff_find_item_id_by_type(file, iinf_start, target)? {
            if let Some(data) = isobmff_read_item_data(file, iloc_start, item_id, idat_content_start)? {
                if is_jpeg(&data) {
                    return Ok(Some(data));
                }
            }
        }
    }

    Ok(None)
}

/// Strategy B: Find a thumbnail track in `moov/trak`.
/// Looks for tracks with `pict` handler type, or `vide` tracks with small dimensions.
/// Reads the first sample from the track's sample table.
fn try_thumbnail_track(file: &mut File) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    let file_end = file.seek(SeekFrom::End(0))?;

    let (moov_start, moov_end) = match isobmff_find_box(file, 0, file_end, b"moov")? {
        Some(v) => v,
        None => return Ok(None),
    };

    // Iterate all trak boxes
    let mut trak_pos = moov_start;
    while trak_pos < moov_end {
        let (trak_start, trak_end) = match isobmff_find_box(file, trak_pos, moov_end, b"trak")? {
            Some(v) => v,
            None => break,
        };

        if let Some(data) = try_extract_from_trak(file, trak_start, trak_end)? {
            if is_jpeg(&data) {
                return Ok(Some(data));
            }
        }

        trak_pos = trak_end;
    }

    Ok(None)
}

/// Check if a trak contains a thumbnail and extract it.
fn try_extract_from_trak(
    file: &mut File,
    trak_start: u64,
    trak_end: u64,
) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    // Find mdia box
    let (mdia_start, mdia_end) = match isobmff_find_box(file, trak_start, trak_end, b"mdia")? {
        Some(v) => v,
        None => return Ok(None),
    };

    // Check handler type
    let (hdlr_start, _hdlr_end) = match isobmff_find_box(file, mdia_start, mdia_end, b"hdlr")? {
        Some(v) => v,
        None => return Ok(None),
    };

    // hdlr is a FullBox: version(1) + flags(3), then pre_defined(4), then handler_type(4)
    file.seek(SeekFrom::Start(hdlr_start + 4 + 4))?;
    let mut handler_type = [0u8; 4];
    file.read_exact(&mut handler_type)?;

    let is_picture_track = &handler_type == b"pict";
    let is_video_track = &handler_type == b"vide";

    if !is_picture_track && !is_video_track {
        return Ok(None);
    }

    // Find minf/stbl
    let (minf_start, minf_end) = match isobmff_find_box(file, mdia_start, mdia_end, b"minf")? {
        Some(v) => v,
        None => return Ok(None),
    };
    let (stbl_start, stbl_end) = match isobmff_find_box(file, minf_start, minf_end, b"stbl")? {
        Some(v) => v,
        None => return Ok(None),
    };

    // For video tracks, check dimensions via stsd to confirm it's a small thumbnail
    if is_video_track {
        if let Some((stsd_start, _stsd_end)) =
            isobmff_find_box(file, stbl_start, stbl_end, b"stsd")?
        {
            // stsd is a FullBox: version(1) + flags(3) + entry_count(4)
            // Then first entry: size(4) + type(4) + reserved(6) + data_ref_index(2) +
            // pre_defined(2) + reserved(2) + pre_defined(12) + width(2) + height(2)
            file.seek(SeekFrom::Start(stsd_start + 4 + 4 + 4 + 4 + 6 + 2 + 2 + 2 + 12))?;
            let mut dims = [0u8; 4];
            file.read_exact(&mut dims)?;
            let width = u16::from_be_bytes([dims[0], dims[1]]);
            let height = u16::from_be_bytes([dims[2], dims[3]]);
            // Only consider small tracks as thumbnails (< 1280 in both dimensions)
            if width > 1280 || height > 1280 {
                return Ok(None);
            }
        }
    }

    // Read first sample: get offset from stco/co64 and size from stsz
    let first_offset = read_first_chunk_offset(file, stbl_start, stbl_end)?;
    let first_size = read_first_sample_size(file, stbl_start, stbl_end)?;

    if let (Some(offset), Some(size)) = (first_offset, first_size) {
        if size > 0 && size < 10_000_000 {
            let data = read_bytes_at(file, offset, size as usize)?;
            return Ok(Some(data));
        }
    }

    Ok(None)
}

/// Read the first chunk offset from stco or co64.
fn read_first_chunk_offset(
    file: &mut File,
    stbl_start: u64,
    stbl_end: u64,
) -> Result<Option<u64>, Box<dyn Error>> {
    // Try stco first (32-bit offsets)
    if let Some((stco_start, _)) = isobmff_find_box(file, stbl_start, stbl_end, b"stco")? {
        // FullBox: version(1) + flags(3) + entry_count(4)
        file.seek(SeekFrom::Start(stco_start + 4))?;
        let mut buf = [0u8; 4];
        file.read_exact(&mut buf)?;
        let count = u32::from_be_bytes(buf);
        if count > 0 {
            file.read_exact(&mut buf)?;
            return Ok(Some(u32::from_be_bytes(buf) as u64));
        }
    }

    // Try co64 (64-bit offsets)
    if let Some((co64_start, _)) = isobmff_find_box(file, stbl_start, stbl_end, b"co64")? {
        file.seek(SeekFrom::Start(co64_start + 4))?;
        let mut buf4 = [0u8; 4];
        file.read_exact(&mut buf4)?;
        let count = u32::from_be_bytes(buf4);
        if count > 0 {
            let mut buf8 = [0u8; 8];
            file.read_exact(&mut buf8)?;
            return Ok(Some(u64::from_be_bytes(buf8)));
        }
    }

    Ok(None)
}

/// Read the first sample size from stsz.
fn read_first_sample_size(
    file: &mut File,
    stbl_start: u64,
    stbl_end: u64,
) -> Result<Option<u64>, Box<dyn Error>> {
    let (stsz_start, _) = match isobmff_find_box(file, stbl_start, stbl_end, b"stsz")? {
        Some(v) => v,
        None => return Ok(None),
    };

    // FullBox: version(1) + flags(3) + sample_size(4) + sample_count(4)
    file.seek(SeekFrom::Start(stsz_start + 4))?;
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf)?;
    let default_size = u32::from_be_bytes(buf);

    file.read_exact(&mut buf)?;
    let sample_count = u32::from_be_bytes(buf);

    if default_size > 0 {
        return Ok(Some(default_size as u64));
    }

    if sample_count > 0 {
        file.read_exact(&mut buf)?;
        return Ok(Some(u32::from_be_bytes(buf) as u64));
    }

    Ok(None)
}

/// Strategy C: Scan `moov/udta` child boxes for embedded JPEG data.
fn try_udta_jpeg(file: &mut File) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    let file_end = file.seek(SeekFrom::End(0))?;

    let (moov_start, moov_end) = match isobmff_find_box(file, 0, file_end, b"moov")? {
        Some(v) => v,
        None => return Ok(None),
    };

    let (udta_start, udta_end) = match isobmff_find_box(file, moov_start, moov_end, b"udta")? {
        Some(v) => v,
        None => return Ok(None),
    };

    // Scan child boxes in udta for JPEG content
    let mut pos = udta_start;
    while pos + 8 <= udta_end {
        file.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 8];
        if file.read(&mut hdr)? < 8 {
            break;
        }
        let size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        let box_end = if size == 0 {
            udta_end
        } else if size == 1 {
            let mut ext = [0u8; 8];
            file.read_exact(&mut ext)?;
            pos + u64::from_be_bytes(ext)
        } else {
            pos + size
        };

        // Check if the box content starts with JPEG SOI
        let content_size = box_end.saturating_sub(pos + 8);
        if content_size > 2 && content_size < 10_000_000 {
            let mut peek = [0u8; 2];
            file.seek(SeekFrom::Start(pos + 8))?;
            if file.read(&mut peek)? == 2 && peek == [0xFF, 0xD8] {
                let data = read_bytes_at(file, pos + 8, content_size as usize)?;
                if is_jpeg(&data) {
                    return Ok(Some(data));
                }
            }
        }

        if box_end <= pos {
            break;
        }
        pos = box_end;
    }

    Ok(None)
}

fn is_jpeg(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8
}

fn read_bytes_at(file: &mut File, offset: u64, len: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(feature = "jpeg")]
fn decode_jpeg_thumbnail(data: &[u8], exif: ExifContext) -> Result<AgnoImage, Box<dyn Error>> {
    let (rgb, width, height) = crate::codec::jpeg::decode_jpeg(data)?;
    Ok(AgnoImage::new(rgb, width as u64, height as u64, exif))
}

#[cfg(not(feature = "jpeg"))]
fn decode_jpeg_thumbnail(_data: &[u8], _exif: ExifContext) -> Result<AgnoImage, Box<dyn Error>> {
    Err("JPEG decoding is not enabled. Please enable the 'jpeg' feature.".into())
}

/// Strategy D: Extract the first HEVC keyframe from the video track and decode
/// it using the native HEVC decoder.
fn try_hevc_keyframe(file: &mut File, exif: ExifContext) -> Result<AgnoImage, Box<dyn Error>> {
    let file_end = file.seek(SeekFrom::End(0))?;

    let (moov_start, moov_end) = isobmff_find_box(file, 0, file_end, b"moov")?
        .ok_or("No moov box")?;

    // Find the video track (handler_type = "vide")
    let mut trak_pos = moov_start;
    while trak_pos < moov_end {
        let (trak_start, trak_end) = match isobmff_find_box(file, trak_pos, moov_end, b"trak")? {
            Some(v) => v,
            None => break,
        };

        if let Ok(Some(img)) = try_decode_hevc_from_trak(file, trak_start, trak_end, &exif) {
            return Ok(img);
        }

        trak_pos = trak_end;
    }

    Err("No decodable HEVC keyframe found".into())
}

/// Try to decode the first keyframe from a single trak, if it's an HEVC video track.
fn try_decode_hevc_from_trak(
    file: &mut File,
    trak_start: u64,
    trak_end: u64,
    exif: &ExifContext,
) -> Result<Option<AgnoImage>, Box<dyn Error>> {
    let (mdia_start, mdia_end) = match isobmff_find_box(file, trak_start, trak_end, b"mdia")? {
        Some(v) => v,
        None => return Ok(None),
    };

    // Check handler type
    let (hdlr_start, _) = match isobmff_find_box(file, mdia_start, mdia_end, b"hdlr")? {
        Some(v) => v,
        None => return Ok(None),
    };
    file.seek(SeekFrom::Start(hdlr_start + 4 + 4))?;
    let mut handler_type = [0u8; 4];
    file.read_exact(&mut handler_type)?;
    if &handler_type != b"vide" {
        return Ok(None);
    }

    let (minf_start, minf_end) = match isobmff_find_box(file, mdia_start, mdia_end, b"minf")? {
        Some(v) => v,
        None => return Ok(None),
    };
    let (stbl_start, stbl_end) = match isobmff_find_box(file, minf_start, minf_end, b"stbl")? {
        Some(v) => v,
        None => return Ok(None),
    };

    // Get hvcC config from stsd
    let (stsd_start, stsd_end) = match isobmff_find_box(file, stbl_start, stbl_end, b"stsd")? {
        Some(v) => v,
        None => return Ok(None),
    };
    let hvcc_data = match extract_hvcc(file, stsd_start, stsd_end)? {
        Some(d) => d,
        None => return Ok(None),
    };

    // Get dimensions from stsd visual sample entry
    let (width, height) = read_stsd_dimensions(file, stsd_start)?;
    if width == 0 || height == 0 {
        return Ok(None);
    }

    // Get first sample offset and size
    let first_offset = read_first_chunk_offset(file, stbl_start, stbl_end)?;
    let first_size = read_first_sample_size(file, stbl_start, stbl_end)?;
    let (offset, size) = match (first_offset, first_size) {
        (Some(o), Some(s)) if s > 0 && s < 50_000_000 => (o, s as usize),
        _ => return Ok(None),
    };

    let sample_data = read_bytes_at(file, offset, size)?;

    // Decode HEVC keyframe natively
    decode_hevc_sample(&hvcc_data, &sample_data, exif.clone()).map(Some)
}

/// Extract the raw hvcC box data (full box content) from stsd > hvc1 > hvcC.
fn extract_hvcc(
    file: &mut File,
    stsd_start: u64,
    stsd_end: u64,
) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    // stsd FullBox: version(1) + flags(3) + entry_count(4)
    let entries_start = stsd_start + 4 + 4;

    // First entry: size(4) + type(4)
    file.seek(SeekFrom::Start(entries_start))?;
    let mut hdr = [0u8; 8];
    file.read_exact(&mut hdr)?;
    let entry_size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
    let entry_type = &hdr[4..8];

    if entry_type != b"hvc1" && entry_type != b"hev1" {
        return Ok(None);
    }

    // Visual sample entry: 78 bytes of fixed fields after the 8-byte box header
    // Then sub-boxes follow
    let sub_boxes_start = entries_start + 8 + 78;
    let entry_end = entries_start + entry_size;
    let search_end = entry_end.min(stsd_end);

    // Find hvcC box
    let (hvcc_start, hvcc_end) =
        match isobmff_find_box(file, sub_boxes_start, search_end, b"hvcC")? {
            Some(v) => v,
            None => return Ok(None),
        };

    let hvcc_len = (hvcc_end - hvcc_start) as usize;
    if hvcc_len > 10_000 {
        return Ok(None);
    }

    let data = read_bytes_at(file, hvcc_start, hvcc_len)?;
    Ok(Some(data))
}

/// Read coded dimensions from the stsd visual sample entry.
fn read_stsd_dimensions(file: &mut File, stsd_start: u64) -> Result<(u16, u16), Box<dyn Error>> {
    // stsd FullBox(4+4) + entry header(8) + reserved(6) + data_ref_index(2) +
    // pre_defined(2) + reserved(2) + pre_defined(12) = 36 bytes to skip, then width(2) + height(2)
    file.seek(SeekFrom::Start(stsd_start + 4 + 4 + 8 + 6 + 2 + 2 + 2 + 12))?;
    let mut dims = [0u8; 4];
    file.read_exact(&mut dims)?;
    Ok((
        u16::from_be_bytes([dims[0], dims[1]]),
        u16::from_be_bytes([dims[2], dims[3]]),
    ))
}

/// Construct a minimal valid HEIF file from an hvcC config and one HEVC sample.
/// Decode HEVC sample data using the native decoder.
fn decode_hevc_sample(
    hvcc_data: &[u8],
    sample_data: &[u8],
    exif: ExifContext,
) -> Result<AgnoImage, Box<dyn Error>> {
    let picture = decode_hevc_still(hvcc_data, sample_data)?;
    let rgb = picture.to_rgb8();
    Ok(AgnoImage::new(
        rgb,
        picture.width as u64,
        picture.height as u64,
        exif,
    ))
}

#[cfg(test)]
mod tests {
    use crate::agno_image::load::{ImageType, detect_image_type, load_agno_image_from_file};
    use std::fs::File;

    #[test]
    fn detect_mov_format() {
        let mut file = File::open("../tests/data/sample.mov").unwrap();
        let image_type = detect_image_type(&mut file).unwrap();
        assert!(
            matches!(image_type, ImageType::QuickTimeMov | ImageType::Mp4),
            "Expected QuickTimeMov or Mp4 for sample.mov"
        );
    }

    #[test]
    fn load_mov_extracts_thumbnail() {
        let img = load_agno_image_from_file("../tests/data/sample.mov").unwrap();
        assert!(img.width > 0, "Width should be non-zero");
        assert!(img.height > 0, "Height should be non-zero");
        assert_eq!(
            img.as_slice().len(),
            (img.width * img.height * 3) as usize,
            "Pixel data length should be width * height * 3"
        );
    }

    #[test]
    fn existing_formats_still_detected() {
        let mut file = File::open("../tests/data/test-heic.heic").unwrap();
        let image_type = detect_image_type(&mut file).unwrap();
        assert!(
            matches!(image_type, ImageType::Heic),
            "HEIC should still be detected as Heic"
        );
    }
}
