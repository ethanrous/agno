//! PDF content stream parser (ISO 32000-1 section 7.8).
//!
//! Content streams use an operand-operator model: operands are pushed onto a
//! stack, then an operator keyword consumes them all.

use std::error::Error;

use super::objects::PdfObject;

/// A single PDF content stream operator with its preceding operands.
#[derive(Debug)]
pub struct Operator {
    pub name: Vec<u8>,
    pub operands: Vec<PdfObject>,
}

const MAX_OPERATORS: usize = 1_000_000;

/// Parse a PDF content stream into a sequence of operators with their operands.
pub fn parse_content_stream(data: &[u8]) -> Result<Vec<Operator>, Box<dyn Error>> {
    let mut ops = Vec::new();
    let mut operand_stack: Vec<PdfObject> = Vec::new();
    let mut lex = super::lexer::Lexer::new(data);

    loop {
        lex.skip_whitespace();
        if lex.at_end() {
            break;
        }

        let b = match lex.peek_byte() {
            Some(b) => b,
            None => break,
        };

        // Operators are bare alphabetic keywords (not starting with '/', not a
        // number). Exceptions: true/false/null are operands, not operators.
        // Also: "'" and '"' are text operators (move-and-show, set-spacing-and-show).
        let is_operator_start = b.is_ascii_alphabetic() || b == b'\'' || b == b'"';

        if is_operator_start {
            let keyword = lex.read_keyword()?;

            if keyword == b"true" {
                operand_stack.push(PdfObject::Bool(true));
            } else if keyword == b"false" {
                operand_stack.push(PdfObject::Bool(false));
            } else if keyword == b"null" {
                operand_stack.push(PdfObject::Null);
            } else if keyword == b"BI" {
                // Inline image: BI <dict pairs> ID <data> EI
                let (dict, data) = parse_inline_image(&mut lex);
                let mut operands = vec![dict, PdfObject::String(data)];
                operands.extend(std::mem::take(&mut operand_stack));
                ops.push(Operator {
                    name: b"BI".to_vec(),
                    operands,
                });
                if ops.len() > MAX_OPERATORS {
                    return Err("Content stream exceeds operator limit".into());
                }
            } else {
                ops.push(Operator {
                    name: keyword,
                    operands: std::mem::take(&mut operand_stack),
                });
                if ops.len() > MAX_OPERATORS {
                    return Err("Content stream exceeds operator limit".into());
                }
            }
        } else {
            // Parse as a PDF object (operand).
            match lex.next_object()? {
                Some(obj) => operand_stack.push(obj),
                None => break,
            }
        }
    }

    Ok(ops)
}

/// Parse an inline image: reads dict pairs until ID, then captures raw data until EI.
/// Returns (dict as PdfObject::Dictionary, raw image bytes).
fn parse_inline_image(lex: &mut super::lexer::Lexer<'_>) -> (PdfObject, Vec<u8>) {
    use std::collections::HashMap;

    // Parse dict key/value pairs until "ID" keyword.
    let mut dict = HashMap::new();
    loop {
        if lex.at_end() { break; }
        lex.skip_whitespace();
        let saved = lex.position();

        // Keys can be either /Name or bare keyword (both forms appear in PDFs).
        // Try reading the next object — if it's a Name, use it as key.
        // If it's a keyword like "ID", we're done with the dict.
        match lex.next_object() {
            Ok(Some(PdfObject::Name(name_bytes))) => {
                let key = expand_inline_key(&name_bytes);
                match lex.next_object() {
                    Ok(Some(val)) => {
                        let val = expand_inline_value(&key, val);
                        dict.insert(key, val);
                    }
                    _ => break,
                }
            }
            Ok(None) => break,
            _ => {
                // Might be a keyword like "ID" — check.
                lex.set_position(saved);
                if let Ok(kw) = lex.read_keyword() {
                    if kw == b"ID" { break; }
                    // Bare keyword as key
                    let key = expand_inline_key(&kw);
                    match lex.next_object() {
                        Ok(Some(val)) => {
                            let val = expand_inline_value(&key, val);
                            dict.insert(key, val);
                        }
                        _ => break,
                    }
                } else {
                    lex.set_position(saved);
                    break;
                }
            }
        }
    }

    // After ID, there's exactly one whitespace byte, then image data until EI.
    let remaining = lex.remaining();
    let data_start = if !remaining.is_empty() && remaining[0].is_ascii_whitespace() { 1 } else { 0 };

    // If we know the image dimensions and BPC, calculate exact data length.
    let expected_len = compute_inline_data_length(&dict);

    if let Some(len) = expected_len {
        // Use exact length: data is len bytes starting at data_start.
        let end = data_start + len;
        if end <= remaining.len() {
            let image_data = remaining[data_start..end].to_vec();
            // Skip past data + whitespace + EI
            let mut pos = end;
            // Skip whitespace before EI
            while pos < remaining.len() && remaining[pos].is_ascii_whitespace() {
                pos += 1;
            }
            // Skip EI
            if pos + 1 < remaining.len() && remaining[pos] == b'E' && remaining[pos + 1] == b'I' {
                pos += 2;
            }
            lex.set_position(lex.position() + pos);
            return (PdfObject::Dictionary(dict), image_data);
        }
    }

    // Fallback: scan for EI preceded by whitespace and followed by delimiter/EOF.
    let bytes = &remaining[data_start..];
    let mut i = 1;
    while i + 1 < bytes.len() {
        if bytes[i] == b'E' && bytes[i + 1] == b'I' {
            let preceded_by_ws = bytes[i - 1].is_ascii_whitespace();
            let followed_by_boundary = i + 2 >= bytes.len()
                || is_content_stream_delimiter(bytes[i + 2]);
            if preceded_by_ws && followed_by_boundary {
                let image_data = remaining[data_start..data_start + i - 1].to_vec();
                lex.set_position(lex.position() + data_start + i + 2);
                return (PdfObject::Dictionary(dict), image_data);
            }
        }
        i += 1;
    }

    // Last resort: consume everything
    lex.set_position(lex.position() + remaining.len());
    (PdfObject::Dictionary(dict), remaining[data_start..].to_vec())
}

/// Compute expected inline image data length from dict dimensions.
/// Returns None if dimensions are unknown or a filter is applied.
fn compute_inline_data_length(dict: &std::collections::HashMap<Vec<u8>, PdfObject>) -> Option<usize> {
    if dict.contains_key(b"Filter".as_slice()) {
        return None;
    }

    let w = dict.get(b"Width".as_slice()).and_then(|v| v.as_i64())? as usize;
    let h = dict.get(b"Height".as_slice()).and_then(|v| v.as_i64())? as usize;
    let bpc = dict.get(b"BitsPerComponent".as_slice())
        .and_then(|v| v.as_i64())
        .unwrap_or(1) as usize;
    let is_mask = dict.get(b"ImageMask".as_slice())
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let components: usize = if is_mask { 1 } else {
        let cs = dict.get(b"ColorSpace".as_slice());
        match cs.and_then(|v| v.as_name_str()) {
            Some("DeviceGray") | Some("G") => 1,
            Some("DeviceRGB") | Some("RGB") => 3,
            Some("DeviceCMYK") | Some("CMYK") => 4,
            _ => 1,
        }
    };

    let row_bits = w * components * bpc;
    let row_bytes = (row_bits + 7) / 8;
    Some(row_bytes * h)
}

/// Expand abbreviated inline image dictionary keys to full names.
fn expand_inline_key(key: &[u8]) -> Vec<u8> {
    match key {
        b"W" => b"Width".to_vec(),
        b"H" => b"Height".to_vec(),
        b"BPC" => b"BitsPerComponent".to_vec(),
        b"CS" => b"ColorSpace".to_vec(),
        b"F" => b"Filter".to_vec(),
        b"DP" => b"DecodeParms".to_vec(),
        b"IM" => b"ImageMask".to_vec(),
        b"I" => b"Interpolate".to_vec(),
        _ => key.to_vec(),
    }
}

/// Expand abbreviated inline image values (color space names, filter names).
fn expand_inline_value(key: &[u8], val: PdfObject) -> PdfObject {
    if key == b"ColorSpace" || key == b"Filter" {
        if let Some(name) = val.as_name_str() {
            let expanded = match name {
                "G" => "DeviceGray",
                "RGB" => "DeviceRGB",
                "CMYK" => "DeviceCMYK",
                "AHx" => "ASCIIHexDecode",
                "A85" => "ASCII85Decode",
                "LZW" => "LZWDecode",
                "Fl" => "FlateDecode",
                "RL" => "RunLengthDecode",
                "CCF" => "CCITTFaxDecode",
                "DCT" => "DCTDecode",
                _ => return val,
            };
            return PdfObject::Name(expanded.as_bytes().to_vec());
        }
    }
    val
}

/// Returns true if the byte is a whitespace or delimiter that can follow `EI`.
#[inline]
fn is_content_stream_delimiter(b: u8) -> bool {
    matches!(
        b,
        0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20
            | b'('
            | b')'
            | b'<'
            | b'>'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'/'
            | b'%'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fill_rect() {
        let ops = parse_content_stream(b"1 0 0 rg 0 0 200 100 re f").unwrap();
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0].name, b"rg");
        assert_eq!(ops[0].operands.len(), 3);
        assert!((ops[0].operands[0].as_f64().unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(ops[1].name, b"re");
        assert_eq!(ops[1].operands.len(), 4);
        assert_eq!(ops[2].name, b"f");
        assert_eq!(ops[2].operands.len(), 0);
    }

    #[test]
    fn parse_text_block() {
        let stream = b"BT /F1 12 Tf 100 700 Td (Hello) Tj ET";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops.len(), 5);
        assert_eq!(ops[0].name, b"BT");
        assert_eq!(ops[1].name, b"Tf");
        assert_eq!(ops[1].operands.len(), 2);
        assert_eq!(ops[2].name, b"Td");
        assert_eq!(ops[3].name, b"Tj");
        assert_eq!(ops[4].name, b"ET");
    }

    #[test]
    fn parse_graphics_state() {
        let stream = b"q 1 0 0 1 50 50 cm 0.5 0.5 0.5 rg 0 0 100 100 re f Q";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops[0].name, b"q");
        assert_eq!(ops[1].name, b"cm");
        assert_eq!(ops[1].operands.len(), 6);
        assert_eq!(ops[5].name, b"Q");
    }

    #[test]
    fn parse_path_operators() {
        let stream = b"100 200 m 200 300 l 200 300 250 350 300 300 c S";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops[0].name, b"m");
        assert_eq!(ops[0].operands.len(), 2);
        assert_eq!(ops[1].name, b"l");
        assert_eq!(ops[1].operands.len(), 2);
        assert_eq!(ops[2].name, b"c");
        assert_eq!(ops[2].operands.len(), 6);
        assert_eq!(ops[3].name, b"S");
    }

    #[test]
    fn empty_stream() {
        let ops = parse_content_stream(b"").unwrap();
        assert!(ops.is_empty());
    }

    #[test]
    fn star_operators() {
        let stream = b"0 0 100 100 re W* f*";
        let ops = parse_content_stream(stream).unwrap();
        assert_eq!(ops[1].name, b"W*");
        assert_eq!(ops[2].name, b"f*");
    }
}
