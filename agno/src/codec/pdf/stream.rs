//! PDF stream filter pipeline (ISO 32000-1 §7.4).
//!
//! Decodes PDF stream data through one or more named filters, applied in order.
//! Supported: FlateDecode (zlib), ASCIIHexDecode, ASCII85Decode, and
//! PNG predictor post-processing for FlateDecode.

use std::error::Error;

use super::objects::PdfObject;

/// Decompress PDF stream data through a filter pipeline.
///
/// `filters` is an ordered list of filter names (e.g., `[b"FlateDecode"]`).
/// `params` is the corresponding `/DecodeParms` for each filter (may be `None`).
/// Filters are applied in the listed order; the output of each becomes the
/// input to the next.
pub fn decode_stream(
    data: &[u8],
    filters: &[&[u8]],
    params: &[Option<&PdfObject>],
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut current = data.to_vec();
    for (i, filter) in filters.iter().enumerate() {
        let p = params.get(i).and_then(|opt| *opt);
        current = apply_filter(&current, filter, p)?;
    }
    Ok(current)
}

/// Apply a single named filter to `data` and return the decoded bytes.
fn apply_filter(
    data: &[u8],
    filter: &[u8],
    params: Option<&PdfObject>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    match filter {
        b"FlateDecode" | b"Fl" => {
            const MAX_DECOMPRESSED_SIZE: usize = 256 * 1024 * 1024; // 256 MB
            let decompressed = miniz_oxide::inflate::decompress_to_vec_zlib(data)
                .map_err(|e| format!("FlateDecode: {e:?}"))?;
            if decompressed.len() > MAX_DECOMPRESSED_SIZE {
                return Err(format!(
                    "FlateDecode: decompressed size {} exceeds limit of {} bytes",
                    decompressed.len(),
                    MAX_DECOMPRESSED_SIZE
                )
                .into());
            }
            apply_png_predictor(decompressed, params)
        }
        b"ASCIIHexDecode" | b"AHx" => decode_ascii_hex(data),
        b"ASCII85Decode" | b"A85" => decode_ascii85(data),
        // DCTDecode (JPEG) and JPXDecode (JPEG2000) are image-specific filters.
        // Pass raw bytes through; image.rs handles decoding.
        b"DCTDecode" | b"DCT" | b"JPXDecode" => Ok(data.to_vec()),
        other => Err(format!(
            "Unsupported PDF stream filter: {}",
            std::str::from_utf8(other).unwrap_or("<non-utf8>")
        )
        .into()),
    }
}

// ---------------------------------------------------------------------------
// FlateDecode PNG predictor (ISO 32000-1 §7.4.4.4, referencing PNG spec)
// ---------------------------------------------------------------------------

/// If the params dict has /Predictor >= 10, reverse the PNG filter rows.
/// Otherwise return `data` unchanged.
fn apply_png_predictor(
    data: Vec<u8>,
    params: Option<&PdfObject>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let predictor = params.and_then(|p| p.get_i64(b"Predictor")).unwrap_or(1);
    if predictor < 10 {
        return Ok(data);
    }

    let columns = params.and_then(|p| p.get_i64(b"Columns")).unwrap_or(1) as usize;
    let colors = params.and_then(|p| p.get_i64(b"Colors")).unwrap_or(1) as usize;
    let bpc = params
        .and_then(|p| p.get_i64(b"BitsPerComponent"))
        .unwrap_or(8) as usize;

    let row_bytes = (columns * colors * bpc + 7) / 8;
    let bytes_per_pixel = (colors * bpc + 7) / 8;
    let stride = 1 + row_bytes; // filter byte + row data

    if data.len() % stride != 0 {
        return Err(format!(
            "PNG predictor: data length {} not divisible by stride {}",
            data.len(),
            stride
        )
        .into());
    }

    let num_rows = data.len() / stride;
    let mut out = vec![0u8; num_rows * row_bytes];
    let mut prev_row = vec![0u8; row_bytes];

    for row in 0..num_rows {
        let filter_byte = data[row * stride];
        let src = &data[row * stride + 1..row * stride + 1 + row_bytes];
        let dst = &mut out[row * row_bytes..(row + 1) * row_bytes];

        match filter_byte {
            0 => {
                // None
                dst.copy_from_slice(src);
            }
            1 => {
                // Sub: add the byte to the left
                for i in 0..row_bytes {
                    let left = if i >= bytes_per_pixel {
                        dst[i - bytes_per_pixel]
                    } else {
                        0
                    };
                    dst[i] = src[i].wrapping_add(left);
                }
            }
            2 => {
                // Up: add the byte above
                for i in 0..row_bytes {
                    dst[i] = src[i].wrapping_add(prev_row[i]);
                }
            }
            3 => {
                // Average: add floor((left + above) / 2)
                for i in 0..row_bytes {
                    let left = if i >= bytes_per_pixel {
                        dst[i - bytes_per_pixel] as u16
                    } else {
                        0
                    };
                    let above = prev_row[i] as u16;
                    dst[i] = src[i].wrapping_add(((left + above) / 2) as u8);
                }
            }
            4 => {
                // Paeth
                for i in 0..row_bytes {
                    let a = if i >= bytes_per_pixel {
                        dst[i - bytes_per_pixel]
                    } else {
                        0
                    };
                    let b = prev_row[i];
                    let c = if i >= bytes_per_pixel {
                        prev_row[i - bytes_per_pixel]
                    } else {
                        0
                    };
                    dst[i] = src[i].wrapping_add(paeth(a, b, c));
                }
            }
            n => {
                return Err(format!("PNG predictor: unknown filter byte {n}").into());
            }
        }

        prev_row.copy_from_slice(dst);
    }

    Ok(out)
}

/// Paeth predictor function per PNG spec / ISO 32000-1 §7.4.4.4.
#[inline]
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i32 + b as i32 - c as i32;
    let pa = (p - a as i32).abs();
    let pb = (p - b as i32).abs();
    let pc = (p - c as i32).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

// ---------------------------------------------------------------------------
// ASCIIHexDecode (ISO 32000-1 §7.4.2)
// ---------------------------------------------------------------------------

/// Decode ASCIIHex-encoded data.
///
/// Hex digit pairs are converted to bytes. Whitespace between digits is
/// ignored. Decoding stops at `>` (EOD marker). An odd trailing nibble is
/// padded with `0`.
fn decode_ascii_hex(data: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut out = Vec::with_capacity(data.len() / 2);
    let mut nibble: Option<u8> = None; // high nibble waiting for low nibble

    for &byte in data {
        if byte == b'>' {
            break;
        }
        if byte.is_ascii_whitespace() {
            continue;
        }
        let value = hex_nibble(byte)?;
        match nibble {
            None => nibble = Some(value),
            Some(high) => {
                out.push((high << 4) | value);
                nibble = None;
            }
        }
    }

    // Odd trailing nibble: pad low nibble with 0
    if let Some(high) = nibble {
        out.push(high << 4);
    }

    Ok(out)
}

/// Convert a single hex character to its 4-bit value.
#[inline]
fn hex_nibble(b: u8) -> Result<u8, Box<dyn Error>> {
    super::util::hex_nibble(b)
        .ok_or_else(|| format!("ASCIIHexDecode: invalid hex character 0x{b:02X}").into())
}

// ---------------------------------------------------------------------------
// ASCII85Decode (ISO 32000-1 §7.4.3)
// ---------------------------------------------------------------------------

/// Decode ASCII85-encoded data.
///
/// Groups of 5 ASCII85 characters (bytes 33–117, `!` to `u`) decode to 4 bytes
/// via base-85. `z` is a shorthand for 4 zero bytes. Decoding stops at `~>`.
/// Whitespace is ignored. A partial final group of 2–4 characters decodes to
/// 1–3 bytes (pad with `u` = 84, then truncate).
fn decode_ascii85(data: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut out = Vec::with_capacity(data.len());
    let mut group: [u8; 5] = [0; 5];
    let mut group_len = 0;
    let mut i = 0;

    while i < data.len() {
        let byte = data[i];
        i += 1;

        // End-of-data marker `~>`
        if byte == b'~' {
            if i < data.len() && data[i] == b'>' {
                break;
            }
            return Err("ASCII85Decode: lone `~` without `>`".into());
        }

        if byte.is_ascii_whitespace() {
            continue;
        }

        if byte == b'z' {
            if group_len != 0 {
                return Err("ASCII85Decode: `z` inside a group".into());
            }
            out.extend_from_slice(&[0u8; 4]);
            continue;
        }

        if !(b'!'..=b'u').contains(&byte) {
            return Err(format!("ASCII85Decode: byte 0x{byte:02X} out of range").into());
        }

        group[group_len] = byte - b'!';
        group_len += 1;

        if group_len == 5 {
            let val = decode_ascii85_group(&group);
            out.extend_from_slice(&val);
            group_len = 0;
        }
    }

    // Partial final group: pad with 84 (`u`), decode, truncate to (group_len - 1) bytes
    if group_len > 0 {
        if group_len < 2 {
            return Err("ASCII85Decode: incomplete final group (need ≥ 2 chars)".into());
        }
        let out_bytes = group_len - 1;
        // Pad remaining slots with 84
        for j in group_len..5 {
            group[j] = 84;
        }
        let val = decode_ascii85_group(&group);
        out.extend_from_slice(&val[..out_bytes]);
    }

    Ok(out)
}

/// Decode a full 5-character ASCII85 group to 4 bytes.
#[inline]
fn decode_ascii85_group(g: &[u8; 5]) -> [u8; 4] {
    let val: u32 = (g[0] as u32) * 85u32.pow(4)
        + (g[1] as u32) * 85u32.pow(3)
        + (g[2] as u32) * 85u32.pow(2)
        + (g[3] as u32) * 85
        + (g[4] as u32);
    val.to_be_bytes()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn flate_decode() {
        let original = b"Hello, PDF world! This is a test of FlateDecode.";
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(original, 6);
        let decoded = decode_stream(&compressed, &[b"FlateDecode"], &[None]).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn ascii_hex_decode() {
        let encoded = b"48656C6C6F>";
        let decoded = decode_stream(encoded, &[b"ASCIIHexDecode"], &[None]).unwrap();
        assert_eq!(decoded, b"Hello");
    }

    #[test]
    fn ascii_hex_with_whitespace() {
        let encoded = b"48 65 6C 6C 6F>";
        let decoded = decode_stream(encoded, &[b"ASCIIHexDecode"], &[None]).unwrap();
        assert_eq!(decoded, b"Hello");
    }

    #[test]
    fn ascii85_decode() {
        let encoded = b"87cURDZ~>";
        let decoded = decode_stream(encoded, &[b"ASCII85Decode"], &[None]).unwrap();
        assert_eq!(decoded, b"Hello");
    }

    #[test]
    fn ascii85_z_shorthand() {
        // 'z' = four zero bytes
        let encoded = b"z~>";
        let decoded = decode_stream(encoded, &[b"ASCII85Decode"], &[None]).unwrap();
        assert_eq!(decoded, &[0u8, 0, 0, 0]);
    }

    #[test]
    fn chained_filters() {
        let original = b"Hello";
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(original, 6);
        // Encode compressed data as hex
        let mut hex = String::new();
        for b in &compressed {
            hex.push_str(&format!("{b:02X}"));
        }
        hex.push('>');

        let decoded = decode_stream(
            hex.as_bytes(),
            &[b"ASCIIHexDecode", b"FlateDecode"],
            &[None, None],
        )
        .unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn flate_with_png_up_predictor() {
        // PNG Up predictor (filter byte 2): each row's values are added to previous row
        // Row width = 3 bytes
        // Row 0: [2, 10, 20, 30] -> decoded [10, 20, 30]
        // Row 1: [2, 5, 5, 5]   -> decoded [10+5, 20+5, 30+5] = [15, 25, 35]
        let filtered = vec![2, 10, 20, 30, 2, 5, 5, 5];
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&filtered, 6);

        let params = PdfObject::Dictionary({
            let mut d = HashMap::new();
            d.insert(b"Predictor".to_vec(), PdfObject::Integer(12));
            d.insert(b"Columns".to_vec(), PdfObject::Integer(3));
            d
        });
        let decoded = decode_stream(&compressed, &[b"FlateDecode"], &[Some(&params)]).unwrap();
        assert_eq!(decoded, vec![10, 20, 30, 15, 25, 35]);
    }

    #[test]
    fn no_filters() {
        let data = b"passthrough";
        let decoded = decode_stream(data, &[], &[]).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn unsupported_filter_error() {
        let result = decode_stream(b"data", &[b"UnknownFilter"], &[None]);
        assert!(result.is_err());
    }
}
