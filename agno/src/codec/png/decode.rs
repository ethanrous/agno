use std::error::Error;

const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

struct IhdrInfo {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace: u8,
}

/// Decode a PNG file from raw bytes, returning (rgb8_data, width, height).
///
/// Supports color types 0 (Grayscale), 2 (RGB), 3 (Indexed), 4 (Gray+Alpha),
/// and 6 (RGBA) at 1/2/4/8/16-bit depths. Non-interlaced only.
pub fn decode_png(data: &[u8]) -> Result<(Vec<u8>, u32, u32), Box<dyn Error>> {
    if data.len() < 8 || data[..8] != PNG_SIGNATURE {
        return Err("Not a valid PNG file".into());
    }

    let mut pos = 8usize;
    let mut ihdr: Option<IhdrInfo> = None;
    let mut palette: Vec<u8> = Vec::new();
    let mut idat_data: Vec<u8> = Vec::new();

    while pos + 12 <= data.len() {
        let chunk_len = read_u32_be(data, pos) as usize;
        let chunk_type = &data[pos + 4..pos + 8];

        let chunk_data_end = pos + 8 + chunk_len;
        if chunk_data_end > data.len() {
            return Err(format!(
                "PNG chunk '{}' at offset {} claims length {} but file is too short",
                String::from_utf8_lossy(chunk_type),
                pos,
                chunk_len
            )
            .into());
        }
        let chunk_data = &data[pos + 8..chunk_data_end];

        match chunk_type {
            b"IHDR" => {
                ihdr = Some(parse_ihdr(chunk_data)?);
            }
            b"PLTE" => {
                if !chunk_len.is_multiple_of(3) {
                    return Err("PLTE chunk length is not a multiple of 3".into());
                }
                palette = chunk_data.to_vec();
            }
            b"IDAT" => {
                idat_data.extend_from_slice(chunk_data);
            }
            b"IEND" => break,
            _ => {} // Skip ancillary/unknown chunks
        }

        // 4(length) + 4(type) + chunk_len(data) + 4(crc)
        pos += 12 + chunk_len;
    }

    let ihdr = ihdr.ok_or("Missing IHDR chunk")?;

    if ihdr.width == 0 || ihdr.height == 0 {
        return Err("PNG has zero width or height".into());
    }

    if ihdr.interlace != 0 {
        return Err("Interlaced PNG (Adam7) is not supported".into());
    }

    if ihdr.color_type == 3 && palette.is_empty() {
        return Err("Indexed color PNG requires a PLTE chunk".into());
    }

    if idat_data.is_empty() {
        return Err("PNG contains no IDAT chunks".into());
    }

    let decompressed = miniz_oxide::inflate::decompress_to_vec_zlib(&idat_data)
        .map_err(|e| format!("PNG zlib decompression failed: {:?}", e))?;

    let raw_pixels = reconstruct_filters(&decompressed, &ihdr)?;

    let rgb = to_rgb8(&raw_pixels, &ihdr, &palette)?;

    Ok((rgb, ihdr.width, ihdr.height))
}

fn read_u32_be(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn parse_ihdr(data: &[u8]) -> Result<IhdrInfo, Box<dyn Error>> {
    if data.len() < 13 {
        return Err("IHDR chunk too short".into());
    }

    let width = read_u32_be(data, 0);
    let height = read_u32_be(data, 4);
    let bit_depth = data[8];
    let color_type = data[9];
    let compression = data[10];
    let filter = data[11];
    let interlace = data[12];

    if compression != 0 {
        return Err(format!("Unsupported PNG compression method: {}", compression).into());
    }
    if filter != 0 {
        return Err(format!("Unsupported PNG filter method: {}", filter).into());
    }
    if interlace > 1 {
        return Err(format!("Invalid PNG interlace method: {}", interlace).into());
    }

    // Validate bit_depth / color_type combinations per PNG spec
    let valid = match color_type {
        0 => matches!(bit_depth, 1 | 2 | 4 | 8 | 16), // Grayscale
        2 => matches!(bit_depth, 8 | 16),             // RGB
        3 => matches!(bit_depth, 1 | 2 | 4 | 8),      // Indexed
        4 => matches!(bit_depth, 8 | 16),             // Gray+Alpha
        6 => matches!(bit_depth, 8 | 16),             // RGBA
        _ => false,
    };
    if !valid {
        return Err(format!(
            "Invalid bit_depth/color_type combination: {}/{}",
            bit_depth, color_type
        )
        .into());
    }

    Ok(IhdrInfo {
        width,
        height,
        bit_depth,
        color_type,
        interlace,
    })
}

/// Bytes per pixel for filter reconstruction. For sub-byte depths this returns 1
/// because the PNG spec defines the filter byte offset `bpp` as max(1, ceil(bits_per_pixel/8)).
fn bytes_per_pixel(ihdr: &IhdrInfo) -> usize {
    let samples_per_pixel: usize = match ihdr.color_type {
        0 => 1, // Grayscale
        2 => 3, // RGB
        3 => 1, // Indexed (palette index)
        4 => 2, // Gray+Alpha
        6 => 4, // RGBA
        _ => 1,
    };
    let bits = samples_per_pixel * (ihdr.bit_depth as usize);
    // bpp for filtering is ceil(bits / 8), minimum 1
    bits.div_ceil(8)
}

/// Byte count for one raw scanline (after filter reconstruction, no filter byte).
fn scanline_bytes(ihdr: &IhdrInfo) -> usize {
    let samples_per_pixel: usize = match ihdr.color_type {
        0 | 3 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => 1,
    };
    let total_bits = (ihdr.width as usize) * samples_per_pixel * (ihdr.bit_depth as usize);
    // Rows are padded to byte boundaries
    total_bits.div_ceil(8)
}

fn reconstruct_filters(decompressed: &[u8], ihdr: &IhdrInfo) -> Result<Vec<u8>, Box<dyn Error>> {
    let h = ihdr.height as usize;
    let stride = scanline_bytes(ihdr);
    let bpp = bytes_per_pixel(ihdr);

    // Each row in the decompressed stream is: 1 filter byte + stride data bytes
    let expected = h * (1 + stride);
    if decompressed.len() < expected {
        return Err(format!(
            "Decompressed PNG data too short: got {} bytes, expected {}",
            decompressed.len(),
            expected
        )
        .into());
    }

    let mut output = vec![0u8; h * stride];
    let mut src_pos = 0;

    for y in 0..h {
        let filter_type = decompressed[src_pos];
        src_pos += 1;

        let row_start = y * stride;
        let prev_row_start = if y > 0 { (y - 1) * stride } else { 0 };

        for x in 0..stride {
            let filtered = decompressed[src_pos + x];

            // a = byte at position x-bpp in current row (0 if x < bpp)
            let a = if x >= bpp {
                output[row_start + x - bpp]
            } else {
                0
            };

            // b = byte at same position in prior row (0 for first row)
            let b = if y > 0 { output[prev_row_start + x] } else { 0 };

            // c = byte at position x-bpp in prior row (0 if first row or x < bpp)
            let c = if y > 0 && x >= bpp {
                output[prev_row_start + x - bpp]
            } else {
                0
            };

            let reconstructed = match filter_type {
                0 => filtered,
                1 => filtered.wrapping_add(a),
                2 => filtered.wrapping_add(b),
                3 => {
                    let avg = ((a as u16 + b as u16) / 2) as u8;
                    filtered.wrapping_add(avg)
                }
                4 => {
                    let pr = paeth_predictor(a, b, c);
                    filtered.wrapping_add(pr)
                }
                _ => return Err(format!("Unknown PNG filter type: {}", filter_type).into()),
            };

            output[row_start + x] = reconstructed;
        }

        src_pos += stride;
    }

    Ok(output)
}

fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    let a_i = a as i32;
    let b_i = b as i32;
    let c_i = c as i32;
    let p = a_i + b_i - c_i;
    let pa = (p - a_i).abs();
    let pb = (p - b_i).abs();
    let pc = (p - c_i).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn to_rgb8(raw: &[u8], ihdr: &IhdrInfo, palette: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    let w = ihdr.width as usize;
    let h = ihdr.height as usize;
    let pixel_count = w * h;
    let mut rgb = vec![0u8; pixel_count * 3];

    match (ihdr.color_type, ihdr.bit_depth) {
        // Grayscale 8-bit
        (0, 8) => {
            for i in 0..pixel_count {
                let g = raw[i];
                rgb[i * 3] = g;
                rgb[i * 3 + 1] = g;
                rgb[i * 3 + 2] = g;
            }
        }
        // Grayscale 16-bit: take high byte
        (0, 16) => {
            for i in 0..pixel_count {
                let g = raw[i * 2];
                rgb[i * 3] = g;
                rgb[i * 3 + 1] = g;
                rgb[i * 3 + 2] = g;
            }
        }
        // Grayscale sub-byte (1, 2, 4 bit)
        (0, bd @ 1) | (0, bd @ 2) | (0, bd @ 4) => {
            unpack_subbyte_grayscale(raw, &mut rgb, w, h, bd)?;
        }
        // RGB 8-bit: direct copy
        (2, 8) => {
            if raw.len() < pixel_count * 3 {
                return Err("Raw data too short for RGB8".into());
            }
            rgb.copy_from_slice(&raw[..pixel_count * 3]);
        }
        // RGB 16-bit: take high byte of each channel
        (2, 16) => {
            for i in 0..pixel_count {
                rgb[i * 3] = raw[i * 6];
                rgb[i * 3 + 1] = raw[i * 6 + 2];
                rgb[i * 3 + 2] = raw[i * 6 + 4];
            }
        }
        // Indexed (palette)
        (3, bd) => {
            let max_index = palette.len() / 3;
            if max_index == 0 {
                return Err("Empty palette for indexed PNG".into());
            }
            if bd == 8 {
                for i in 0..pixel_count {
                    let idx = raw[i] as usize;
                    if idx >= max_index {
                        return Err(format!(
                            "Palette index {} out of range (palette has {} entries)",
                            idx, max_index
                        )
                        .into());
                    }
                    rgb[i * 3] = palette[idx * 3];
                    rgb[i * 3 + 1] = palette[idx * 3 + 1];
                    rgb[i * 3 + 2] = palette[idx * 3 + 2];
                }
            } else {
                // Sub-byte indexed (1, 2, 4 bit)
                unpack_subbyte_indexed(raw, &mut rgb, w, h, bd, palette, max_index)?;
            }
        }
        // Gray+Alpha 8-bit: take gray, discard alpha
        (4, 8) => {
            for i in 0..pixel_count {
                let g = raw[i * 2];
                rgb[i * 3] = g;
                rgb[i * 3 + 1] = g;
                rgb[i * 3 + 2] = g;
            }
        }
        // Gray+Alpha 16-bit: take high byte of gray, discard alpha
        (4, 16) => {
            for i in 0..pixel_count {
                let g = raw[i * 4];
                rgb[i * 3] = g;
                rgb[i * 3 + 1] = g;
                rgb[i * 3 + 2] = g;
            }
        }
        // RGBA 8-bit: take R, G, B, discard alpha
        (6, 8) => {
            for i in 0..pixel_count {
                rgb[i * 3] = raw[i * 4];
                rgb[i * 3 + 1] = raw[i * 4 + 1];
                rgb[i * 3 + 2] = raw[i * 4 + 2];
            }
        }
        // RGBA 16-bit: take high byte of R, G, B, discard alpha
        (6, 16) => {
            for i in 0..pixel_count {
                rgb[i * 3] = raw[i * 8];
                rgb[i * 3 + 1] = raw[i * 8 + 2];
                rgb[i * 3 + 2] = raw[i * 8 + 4];
            }
        }
        _ => {
            return Err(format!(
                "Unsupported color_type/bit_depth: {}/{}",
                ihdr.color_type, ihdr.bit_depth
            )
            .into());
        }
    }

    Ok(rgb)
}

/// Unpack sub-byte grayscale (1, 2, or 4 bit) pixels and scale to 0-255.
fn unpack_subbyte_grayscale(
    raw: &[u8],
    rgb: &mut [u8],
    width: usize,
    height: usize,
    bit_depth: u8,
) -> Result<(), Box<dyn Error>> {
    let bd = bit_depth as usize;
    let pixels_per_byte = 8 / bd;
    let mask = (1u8 << bd) - 1;
    let max_val = mask as f32;
    let stride = (width * bd).div_ceil(8);

    for y in 0..height {
        for x in 0..width {
            let byte_index = y * stride + x / pixels_per_byte;
            let bit_offset = (pixels_per_byte - 1 - x % pixels_per_byte) * bd;
            let sample = (raw[byte_index] >> bit_offset) & mask;
            let scaled = ((sample as f32 / max_val) * 255.0 + 0.5) as u8;
            let pixel_idx = (y * width + x) * 3;
            rgb[pixel_idx] = scaled;
            rgb[pixel_idx + 1] = scaled;
            rgb[pixel_idx + 2] = scaled;
        }
    }

    Ok(())
}

/// Unpack sub-byte indexed (1, 2, or 4 bit) pixels and look up palette.
fn unpack_subbyte_indexed(
    raw: &[u8],
    rgb: &mut [u8],
    width: usize,
    height: usize,
    bit_depth: u8,
    palette: &[u8],
    max_index: usize,
) -> Result<(), Box<dyn Error>> {
    let bd = bit_depth as usize;
    let pixels_per_byte = 8 / bd;
    let mask = (1u8 << bd) - 1;
    let stride = (width * bd).div_ceil(8);

    for y in 0..height {
        for x in 0..width {
            let byte_index = y * stride + x / pixels_per_byte;
            let bit_offset = (pixels_per_byte - 1 - x % pixels_per_byte) * bd;
            let idx = ((raw[byte_index] >> bit_offset) & mask) as usize;
            if idx >= max_index {
                return Err(format!(
                    "Palette index {} out of range (palette has {} entries)",
                    idx, max_index
                )
                .into());
            }
            let pixel_idx = (y * width + x) * 3;
            rgb[pixel_idx] = palette[idx * 3];
            rgb[pixel_idx + 1] = palette[idx * 3 + 1];
            rgb[pixel_idx + 2] = palette[idx * 3 + 2];
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_signature() {
        let data = [0u8; 16];
        let result = decode_png(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Not a valid PNG"));
    }

    #[test]
    fn test_truncated_file() {
        let result = decode_png(&PNG_SIGNATURE);
        assert!(result.is_err());
    }

    #[test]
    fn test_paeth_predictor_values() {
        // When a=b=c=0, should return 0
        assert_eq!(paeth_predictor(0, 0, 0), 0);
        // When a=1, b=2, c=3: p=0, pa=1, pb=2, pc=3 -> a
        assert_eq!(paeth_predictor(1, 2, 3), 1);
        // When a=10, b=20, c=10: p=20, pa=10, pb=0, pc=10 -> b
        assert_eq!(paeth_predictor(10, 20, 10), 20);
        // When a=0, b=0, c=1: p=-1, pa=1, pb=1, pc=2 -> a (pa<=pb)
        assert_eq!(paeth_predictor(0, 0, 1), 0);
    }

    #[test]
    fn test_filter_none() {
        // 1x2 grayscale 8-bit: filter_type=0 (None)
        let ihdr = IhdrInfo {
            width: 2,
            height: 1,
            bit_depth: 8,
            color_type: 0,
            interlace: 0,
        };
        // filter_byte=0, then 2 data bytes
        let decompressed = vec![0, 100, 200];
        let result = reconstruct_filters(&decompressed, &ihdr).unwrap();
        assert_eq!(result, vec![100, 200]);
    }

    #[test]
    fn test_filter_sub() {
        // 3x1 grayscale 8-bit, filter_type=1 (Sub), bpp=1
        // filtered: [5, 3, 7] -> raw: [5, 5+3=8, 8+7=15]
        let ihdr = IhdrInfo {
            width: 3,
            height: 1,
            bit_depth: 8,
            color_type: 0,
            interlace: 0,
        };
        let decompressed = vec![1, 5, 3, 7];
        let result = reconstruct_filters(&decompressed, &ihdr).unwrap();
        assert_eq!(result, vec![5, 8, 15]);
    }

    #[test]
    fn test_filter_up() {
        // 2x2 grayscale 8-bit, both rows filter_type=2 (Up)
        // Row 0: prior=0, filtered=[10,20] -> raw=[10,20]
        // Row 1: prior=[10,20], filtered=[5,5] -> raw=[15,25]
        let ihdr = IhdrInfo {
            width: 2,
            height: 2,
            bit_depth: 8,
            color_type: 0,
            interlace: 0,
        };
        let decompressed = vec![2, 10, 20, 2, 5, 5];
        let result = reconstruct_filters(&decompressed, &ihdr).unwrap();
        assert_eq!(result, vec![10, 20, 15, 25]);
    }

    #[test]
    fn test_filter_average() {
        // 2x2 grayscale 8-bit, filter_type=3 (Average)
        // Row 0: a=0,b=0 for first; a=prev,b=0 for rest
        //   filtered=[10, 20] -> raw[0]=10+floor(0/2)=10, raw[1]=20+floor(10/2)=25
        // Row 1: a=0,b=10 for x=0; a=prev,b=25 for x=1
        //   filtered=[3, 7] -> raw[0]=3+floor((0+10)/2)=8, raw[1]=7+floor((8+25)/2)=23
        let ihdr = IhdrInfo {
            width: 2,
            height: 2,
            bit_depth: 8,
            color_type: 0,
            interlace: 0,
        };
        let decompressed = vec![3, 10, 20, 3, 3, 7];
        let result = reconstruct_filters(&decompressed, &ihdr).unwrap();
        assert_eq!(result, vec![10, 25, 8, 23]);
    }

    #[test]
    fn test_grayscale_to_rgb() {
        let ihdr = IhdrInfo {
            width: 2,
            height: 1,
            bit_depth: 8,
            color_type: 0,
            interlace: 0,
        };
        let raw = vec![100, 200];
        let rgb = to_rgb8(&raw, &ihdr, &[]).unwrap();
        assert_eq!(rgb, vec![100, 100, 100, 200, 200, 200]);
    }

    #[test]
    fn test_rgba_to_rgb() {
        let ihdr = IhdrInfo {
            width: 1,
            height: 1,
            bit_depth: 8,
            color_type: 6,
            interlace: 0,
        };
        let raw = vec![10, 20, 30, 255]; // R=10, G=20, B=30, A=255
        let rgb = to_rgb8(&raw, &ihdr, &[]).unwrap();
        assert_eq!(rgb, vec![10, 20, 30]);
    }

    #[test]
    fn test_indexed_requires_palette() {
        let data_with_no_plte = build_minimal_png(1, 1, 8, 3, &[], &[0, 0]);
        let result = decode_png(&data_with_no_plte);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("PLTE"));
    }

    #[test]
    fn test_zero_dimension() {
        // Build a PNG with width=0
        let mut buf = Vec::new();
        buf.extend_from_slice(&PNG_SIGNATURE);
        // IHDR: width=0, height=1, depth=8, color=2, comp=0, filt=0, interlace=0
        let ihdr_data: [u8; 13] = [0, 0, 0, 0, 0, 0, 0, 1, 8, 2, 0, 0, 0];
        write_chunk(&mut buf, b"IHDR", &ihdr_data);
        write_chunk(&mut buf, b"IEND", &[]);
        let result = decode_png(&buf);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("zero"));
    }

    #[test]
    fn test_interlaced_rejected() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&PNG_SIGNATURE);
        // interlace = 1 (Adam7)
        let ihdr_data: [u8; 13] = [0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 1];
        write_chunk(&mut buf, b"IHDR", &ihdr_data);
        write_chunk(&mut buf, b"IEND", &[]);
        let result = decode_png(&buf);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Interlaced"));
    }

    #[test]
    fn test_roundtrip_rgb8_1x1() {
        // Build a minimal valid 1x1 RGB8 PNG with filter=None
        let scanline = vec![0u8, 255, 0, 128]; // filter=0, R=255, G=0, B=128
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&scanline, 6);
        let palette: &[u8] = &[];
        let png = build_minimal_png(1, 1, 8, 2, palette, &compressed);

        let (rgb, w, h) = decode_png(&png).unwrap();
        assert_eq!(w, 1);
        assert_eq!(h, 1);
        assert_eq!(rgb, vec![255, 0, 128]);
    }

    #[test]
    fn test_roundtrip_indexed_2x1() {
        // 2x1, indexed, 8-bit, palette = [(255,0,0), (0,255,0)]
        let palette = vec![255, 0, 0, 0, 255, 0];
        // filter=0, index 0, index 1
        let scanline = vec![0u8, 0, 1];
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&scanline, 6);
        let png = build_minimal_png(2, 1, 8, 3, &palette, &compressed);

        let (rgb, w, h) = decode_png(&png).unwrap();
        assert_eq!(w, 2);
        assert_eq!(h, 1);
        assert_eq!(rgb, vec![255, 0, 0, 0, 255, 0]);
    }

    #[test]
    fn test_roundtrip_rgba8_1x1() {
        // 1x1 RGBA8 with alpha=128
        let scanline = vec![0u8, 10, 20, 30, 128]; // filter=0, R=10, G=20, B=30, A=128
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&scanline, 6);
        let png = build_minimal_png(1, 1, 8, 6, &[], &compressed);

        let (rgb, w, h) = decode_png(&png).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(rgb, vec![10, 20, 30]);
    }

    #[test]
    fn test_roundtrip_gray_alpha_8bit() {
        // 1x1 Gray+Alpha
        let scanline = vec![0u8, 200, 128]; // filter=0, gray=200, alpha=128
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&scanline, 6);
        let png = build_minimal_png(1, 1, 8, 4, &[], &compressed);

        let (rgb, w, h) = decode_png(&png).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(rgb, vec![200, 200, 200]);
    }

    #[test]
    fn test_roundtrip_rgb16_1x1() {
        // 1x1 RGB 16-bit. Each channel is 2 bytes big-endian.
        // R=0xAB00, G=0xCD00, B=0xEF00 -> high bytes: 0xAB, 0xCD, 0xEF
        let scanline = vec![0u8, 0xAB, 0x00, 0xCD, 0x00, 0xEF, 0x00];
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&scanline, 6);
        let png = build_minimal_png(1, 1, 16, 2, &[], &compressed);

        let (rgb, w, h) = decode_png(&png).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(rgb, vec![0xAB, 0xCD, 0xEF]);
    }

    #[test]
    fn test_filter_sub_rgb8() {
        // 2x1 RGB8 with Sub filter. bpp=3
        // filtered: [10,20,30, 5,5,5] -> raw: [10,20,30, 15,25,35]
        let scanline = vec![1u8, 10, 20, 30, 5, 5, 5];
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&scanline, 6);
        let png = build_minimal_png(2, 1, 8, 2, &[], &compressed);

        let (rgb, w, h) = decode_png(&png).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgb, vec![10, 20, 30, 15, 25, 35]);
    }

    #[test]
    fn test_multiple_rows_with_different_filters() {
        // 2x2 RGB8
        // Row 0: filter=0 (None), pixels: [10,20,30, 40,50,60]
        // Row 1: filter=2 (Up),   filtered: [1,2,3, 4,5,6]
        //   -> raw: [10+1, 20+2, 30+3, 40+4, 50+5, 60+6] = [11,22,33, 44,55,66]
        let mut scanlines = Vec::new();
        scanlines.push(0u8); // filter=None
        scanlines.extend_from_slice(&[10, 20, 30, 40, 50, 60]);
        scanlines.push(2u8); // filter=Up
        scanlines.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&scanlines, 6);
        let png = build_minimal_png(2, 2, 8, 2, &[], &compressed);

        let (rgb, w, h) = decode_png(&png).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(rgb, vec![10, 20, 30, 40, 50, 60, 11, 22, 33, 44, 55, 66]);
    }

    // --- Test helpers ---

    fn build_minimal_png(
        width: u32,
        height: u32,
        bit_depth: u8,
        color_type: u8,
        palette: &[u8],
        compressed_idat: &[u8],
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&PNG_SIGNATURE);

        // IHDR
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(bit_depth);
        ihdr.push(color_type);
        ihdr.push(0); // compression
        ihdr.push(0); // filter
        ihdr.push(0); // interlace = none
        write_chunk(&mut buf, b"IHDR", &ihdr);

        if !palette.is_empty() {
            write_chunk(&mut buf, b"PLTE", palette);
        }

        write_chunk(&mut buf, b"IDAT", compressed_idat);
        write_chunk(&mut buf, b"IEND", &[]);

        buf
    }

    fn write_chunk(buf: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
        buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
        buf.extend_from_slice(chunk_type);
        buf.extend_from_slice(data);
        // Write a dummy CRC (we don't validate it)
        buf.extend_from_slice(&[0, 0, 0, 0]);
    }
}
