//! PDF cross-reference table and stream parser (ISO 32000-1 §7.5.4, §7.5.8).
//!
//! Handles traditional xref tables (PDF 1.0+), compressed xref streams (PDF 1.5+),
//! linearized PDFs, and /Prev chain following.

use std::collections::HashMap;
use std::error::Error;

use super::lexer::Lexer;
use super::objects::PdfObject;

/// Maximum depth for /Prev chain traversal to prevent infinite loops.
const MAX_PREV_DEPTH: usize = 64;

/// Entry in the cross-reference table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum XrefEntry {
    InUse { offset: u64, generation: u16 },
    Free,
    Compressed { stream_obj: u32, index: u16 },
}

/// Parsed cross-reference data for the entire PDF.
pub struct Xref {
    pub entries: HashMap<u32, XrefEntry>,
    pub trailer: PdfObject,
}

/// Parse all cross-reference data from a PDF file.
pub fn parse_xref(data: &[u8]) -> Result<Xref, Box<dyn Error>> {
    let start_offset = find_startxref(data)?;
    let mut entries: HashMap<u32, XrefEntry> = HashMap::new();
    let mut trailer: Option<PdfObject> = None;

    parse_xref_chain(data, start_offset, &mut entries, &mut trailer, 0)?;

    let trailer = trailer.ok_or("No trailer found in PDF")?;
    Ok(Xref { entries, trailer })
}

/// Recursively (iteratively bounded) parse an xref section and follow /Prev.
///
/// Entries already in `entries` are NOT overwritten — the most recently parsed
/// section (first call) takes priority over older sections (/Prev chain).
fn parse_xref_chain(
    data: &[u8],
    offset: u64,
    entries: &mut HashMap<u32, XrefEntry>,
    trailer: &mut Option<PdfObject>,
    depth: usize,
) -> Result<(), Box<dyn Error>> {
    if depth > MAX_PREV_DEPTH {
        return Err("PDF xref /Prev chain too deep (possible loop)".into());
    }

    let pos = offset as usize;
    if pos >= data.len() {
        return Err(format!(
            "xref offset {offset} is beyond end of file ({} bytes)",
            data.len()
        )
        .into());
    }

    // Determine whether this is a traditional table or a compressed stream.
    // A traditional xref starts with the literal bytes `xref`.
    let this_trailer = if data[pos..].starts_with(b"xref") {
        parse_traditional_xref(data, pos, entries)?
    } else {
        parse_xref_stream(data, pos, entries)?
    };

    // Extract /Prev before potentially overwriting the trailer.
    let prev_offset = this_trailer.get_i64(b"Prev").map(|n| n as u64);

    // Only store the first (most recent) trailer.
    if trailer.is_none() {
        *trailer = Some(this_trailer);
    }

    // Follow /Prev chain.
    if let Some(prev) = prev_offset {
        parse_xref_chain(data, prev, entries, trailer, depth + 1)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Traditional xref table
// ---------------------------------------------------------------------------

/// Parse a traditional xref table starting at `pos` and return the trailer dict.
///
/// Format (ISO 32000-1 §7.5.4):
/// ```text
/// xref
/// <first> <count>
/// <10-digit offset> <5-digit gen> <'f'|'n'> <EOL>  (20 bytes per entry)
/// ...
/// trailer
/// << ... >>
/// ```
fn parse_traditional_xref(
    data: &[u8],
    pos: usize,
    entries: &mut HashMap<u32, XrefEntry>,
) -> Result<PdfObject, Box<dyn Error>> {
    let mut cur = pos;

    // Skip "xref" keyword and the following EOL.
    cur += 4; // b"xref"
    skip_eol(data, &mut cur);

    // Parse subsections until we see "trailer".
    loop {
        // Skip any whitespace between subsections.
        while cur < data.len() && is_ascii_whitespace(data[cur]) {
            cur += 1;
        }
        if cur >= data.len() {
            return Err("Unexpected end of file in xref table".into());
        }

        // Check for "trailer" keyword (may be followed by delimiter).
        if data[cur..].starts_with(b"trailer") {
            let after = cur + 7;
            if after >= data.len() || is_delimiter_or_ws(data[after]) {
                cur = after;
                break;
            }
        }

        // Parse subsection header: "<first> <count>\n"
        let first_obj = parse_ascii_u32(data, &mut cur)?;
        skip_whitespace_inline(data, &mut cur);
        let count = parse_ascii_u32(data, &mut cur)? as usize;
        skip_eol(data, &mut cur);

        // Parse exactly `count` 20-byte entries.
        for i in 0..count {
            let obj_num = first_obj + i as u32;
            let entry = parse_xref_entry(data, &mut cur)?;
            // Do NOT overwrite — newer (earlier in chain) entries take priority.
            entries.entry(obj_num).or_insert(entry);
        }
    }

    // Parse trailer dictionary.
    skip_whitespace_inline(data, &mut cur);
    let mut lexer = Lexer::new(data);
    lexer.set_position(cur);
    let trailer = lexer
        .next_object()?
        .ok_or("Expected trailer dictionary after 'trailer' keyword")?;

    if trailer.as_dict().is_none() {
        return Err(format!("Trailer is not a dictionary: {trailer}").into());
    }

    Ok(trailer)
}

/// Parse a single 20-byte xref entry.
///
/// Format: `OOOOOOOOOO GGGGG N/F EOL` where EOL is exactly 2 bytes.
/// ISO 32000-1 §7.5.4: each entry is exactly 20 bytes.
fn parse_xref_entry(data: &[u8], cur: &mut usize) -> Result<XrefEntry, Box<dyn Error>> {
    let entry_start = *cur;

    if *cur + 20 > data.len() {
        return Err(format!(
            "Truncated xref entry at offset {} (file length {})",
            cur,
            data.len()
        )
        .into());
    }

    // The 20-byte format is: 10 digits, space, 5 digits, space, 'f'/'n', EOL (2 bytes).
    // We parse the numeric fields flexibly to handle minor formatting variations.
    let offset_or_next = parse_ascii_u64_fixed(data, cur, 10)?;
    if *cur < data.len() && data[*cur] == b' ' {
        *cur += 1;
    }
    let generation = parse_ascii_u32_fixed(data, cur, 5)? as u16;
    if *cur < data.len() && data[*cur] == b' ' {
        *cur += 1;
    }

    let kind = data
        .get(*cur)
        .copied()
        .ok_or("Truncated xref entry: missing type byte")?;
    *cur += 1;

    // Always advance to entry_start + 20 to maintain alignment.
    // This handles all EOL variants: "\r\n", " \n", "\r", "\n".
    *cur = entry_start + 20;

    match kind {
        b'f' => Ok(XrefEntry::Free),
        b'n' => Ok(XrefEntry::InUse {
            offset: offset_or_next,
            generation,
        }),
        other => Err(format!("Invalid xref entry type byte: 0x{other:02X}").into()),
    }
}

// ---------------------------------------------------------------------------
// Compressed xref stream (PDF 1.5+)
// ---------------------------------------------------------------------------

/// Parse an xref stream object at `pos` and return the combined trailer/stream dict.
///
/// ISO 32000-1 §7.5.8: the stream dictionary IS the trailer dictionary.
fn parse_xref_stream(
    data: &[u8],
    pos: usize,
    entries: &mut HashMap<u32, XrefEntry>,
) -> Result<PdfObject, Box<dyn Error>> {
    let mut lexer = Lexer::new(data);
    lexer.set_position(pos);

    // Read: `N 0 obj`
    let obj_num = lexer
        .next_object()?
        .and_then(|o| o.as_i64())
        .ok_or("xref stream: expected object number")?;
    let _gen = lexer
        .next_object()?
        .and_then(|o| o.as_i64())
        .ok_or("xref stream: expected generation number")?;
    lexer.expect_keyword(b"obj")?;

    // Read the stream dictionary.
    let dict_obj = lexer
        .next_object()?
        .ok_or("xref stream: expected dictionary")?;
    if dict_obj.as_dict().is_none() {
        return Err(
            format!("xref stream object {obj_num}: expected dictionary, got {dict_obj}").into(),
        );
    }

    // Find `stream` keyword, then read raw stream bytes.
    lexer.skip_whitespace();
    let kw = lexer.read_keyword()?;
    if kw != b"stream" {
        return Err(format!(
            "xref stream: expected 'stream', got {:?}",
            std::str::from_utf8(&kw)
        )
        .into());
    }

    // After `stream` the spec requires a single newline (LF or CR LF).
    let stream_start = skip_stream_newline(data, lexer.position());

    // Get /Length from dictionary to know how many bytes to read.
    // Fall back to scanning for `endstream` when /Length is missing, indirect, or out of bounds.
    let raw_stream = match dict_obj.get_i64(b"Length") {
        Some(length) if length >= 0 && stream_start + length as usize <= data.len() => {
            &data[stream_start..stream_start + length as usize]
        }
        _ => {
            let marker = b"endstream";
            let search_end = data.len().saturating_sub(marker.len());
            let mut end_pos = None;
            let mut pos = stream_start;
            while pos <= search_end {
                if &data[pos..pos + marker.len()] == marker {
                    end_pos = Some(pos);
                    break;
                }
                pos += 1;
            }
            let end =
                end_pos.ok_or("xref stream: /Length invalid and could not locate 'endstream'")?;
            let mut trimmed = end;
            while trimmed > stream_start && data[trimmed - 1].is_ascii_whitespace() {
                trimmed -= 1;
            }
            &data[stream_start..trimmed]
        }
    };

    // Decompress using the full stream filter pipeline (handles FlateDecode + predictor).
    let filters: Vec<&[u8]> = match dict_obj.get(b"Filter") {
        Some(PdfObject::Name(n)) => vec![n.as_slice()],
        Some(PdfObject::Array(arr)) => arr.iter().filter_map(|o| o.as_name()).collect(),
        _ => vec![],
    };
    let params: Vec<Option<&PdfObject>> = match dict_obj.get(b"DecodeParms") {
        Some(p) if matches!(p, PdfObject::Dictionary(_)) => vec![Some(p)],
        Some(PdfObject::Array(arr)) => arr.iter().map(Some).collect(),
        _ => filters.iter().map(|_| None).collect(),
    };
    let decompressed = super::stream::decode_stream(raw_stream, &filters, &params)?;

    // Decode entries using /W (field widths) and /Index (subsections).
    let w_arr = dict_obj
        .get(b"W")
        .and_then(|o| o.as_array())
        .ok_or("xref stream missing /W array")?;
    if w_arr.len() != 3 {
        return Err(format!("xref stream /W must have 3 elements, got {}", w_arr.len()).into());
    }
    let w: [usize; 3] = [
        w_arr[0].as_i64().unwrap_or(0) as usize,
        w_arr[1].as_i64().ok_or("xref stream /W[1] missing")? as usize,
        w_arr[2].as_i64().unwrap_or(0) as usize,
    ];
    let entry_size = w[0] + w[1] + w[2];
    if entry_size == 0 {
        return Err("xref stream /W entry size is 0".into());
    }

    // /Index defines the (first, count) subsection pairs; default is [0 /Size].
    let size = dict_obj.get_i64(b"Size").unwrap_or(0) as u32;
    let index_pairs: Vec<(u32, u32)> =
        if let Some(idx) = dict_obj.get(b"Index").and_then(|o| o.as_array()) {
            if idx.len() % 2 != 0 {
                return Err(
                    format!("xref stream /Index must have even count, got {}", idx.len()).into(),
                );
            }
            idx.chunks(2)
                .map(|chunk| {
                    let first = chunk[0].as_i64().unwrap_or(0) as u32;
                    let count = chunk[1].as_i64().unwrap_or(0) as u32;
                    (first, count)
                })
                .collect()
        } else {
            vec![(0, size)]
        };

    // Decode all entries.
    let mut stream_pos = 0usize;
    for (first_obj, count) in index_pairs {
        for i in 0..count {
            let obj_num = first_obj + i;
            if stream_pos + entry_size > decompressed.len() {
                return Err(format!(
                    "xref stream data too short at entry {obj_num} (pos {stream_pos}, need {entry_size}, have {})",
                    decompressed.len()
                )
                .into());
            }

            let type_val = if w[0] == 0 {
                1u64 // default type is 1 (InUse) when w1==0
            } else {
                read_be_uint(&decompressed[stream_pos..stream_pos + w[0]])
            };
            let field2 = read_be_uint(&decompressed[stream_pos + w[0]..stream_pos + w[0] + w[1]]);
            let field3 = if w[2] == 0 {
                0u64
            } else {
                read_be_uint(&decompressed[stream_pos + w[0] + w[1]..stream_pos + entry_size])
            };
            stream_pos += entry_size;

            let entry = match type_val {
                0 => XrefEntry::Free,
                1 => XrefEntry::InUse {
                    offset: field2,
                    generation: field3 as u16,
                },
                2 => XrefEntry::Compressed {
                    stream_obj: field2 as u32,
                    index: field3 as u16,
                },
                other => {
                    // Unknown types are skipped per spec.
                    tracing::debug!(
                        "Unknown xref stream entry type {other} for obj {obj_num}, skipping"
                    );
                    continue;
                }
            };

            // Do NOT overwrite — newer entries take priority.
            entries.entry(obj_num).or_insert(entry);
        }
    }

    Ok(dict_obj)
}

// ---------------------------------------------------------------------------
// startxref search
// ---------------------------------------------------------------------------

/// Find the byte offset indicated by the last `startxref` keyword in the file.
///
/// Scans backward from the end of the file (up to 1024 bytes) to find
/// the LAST occurrence of `startxref`, then parses the integer following it.
fn find_startxref(data: &[u8]) -> Result<u64, Box<dyn Error>> {
    // Search the last 1024 bytes (or the whole file if smaller).
    let search_start = data.len().saturating_sub(1024);
    let tail = &data[search_start..];

    // Find the last occurrence of `startxref` in the tail.
    let token = b"startxref";
    let mut last_found: Option<usize> = None;
    let mut search_pos = 0usize;
    while let Some(rel) = find_bytes(tail, search_pos, token) {
        last_found = Some(rel);
        search_pos = rel + 1;
    }

    let rel_pos = last_found.ok_or("PDF file has no 'startxref' keyword")?;
    let abs_pos = search_start + rel_pos + token.len();

    // Parse the integer on the line(s) following `startxref`.
    let mut cur = abs_pos;
    skip_whitespace_bytes(data, &mut cur);

    let offset = parse_ascii_u64(data, &mut cur)?;
    Ok(offset)
}

/// Find the first occurrence of `needle` in `haystack[from..]`.
/// Returns the index relative to `haystack` start (not `from`).
fn find_bytes(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    let limit = haystack.len() - needle.len() + 1;
    (from..limit).find(|&i| haystack[i..i + needle.len()] == *needle)
}

// ---------------------------------------------------------------------------
// Low-level parsing helpers
// ---------------------------------------------------------------------------

/// Skip a line ending (LF, CR, or CR+LF) at `*cur`.
fn skip_eol(data: &[u8], cur: &mut usize) {
    if *cur < data.len() && data[*cur] == b'\r' {
        *cur += 1;
    }
    if *cur < data.len() && data[*cur] == b'\n' {
        *cur += 1;
    }
}

/// Skip the mandatory newline immediately after the `stream` keyword.
/// Returns the position of the first stream byte.
fn skip_stream_newline(data: &[u8], pos: usize) -> usize {
    let mut cur = pos;
    if cur < data.len() && data[cur] == b'\r' {
        cur += 1;
    }
    if cur < data.len() && data[cur] == b'\n' {
        cur += 1;
    }
    cur
}

/// Skip inline whitespace (space and horizontal tab) at `*cur`.
fn skip_whitespace_inline(data: &[u8], cur: &mut usize) {
    while *cur < data.len() && matches!(data[*cur], b' ' | b'\t') {
        *cur += 1;
    }
}

/// Skip all PDF whitespace (NUL, TAB, LF, FF, CR, SPACE) at `*cur`.
fn skip_whitespace_bytes(data: &[u8], cur: &mut usize) {
    while *cur < data.len() && is_ascii_whitespace(data[*cur]) {
        *cur += 1;
    }
}

#[inline]
fn is_ascii_whitespace(b: u8) -> bool {
    matches!(b, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

#[inline]
fn is_delimiter_or_ws(b: u8) -> bool {
    is_ascii_whitespace(b)
        || matches!(
            b,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        )
}

/// Parse an ASCII decimal unsigned 32-bit integer at `*cur`, advancing past it.
fn parse_ascii_u32(data: &[u8], cur: &mut usize) -> Result<u32, Box<dyn Error>> {
    let start = *cur;
    while *cur < data.len() && data[*cur].is_ascii_digit() {
        *cur += 1;
    }
    if *cur == start {
        return Err(format!("Expected decimal integer at offset {start}").into());
    }
    let s = std::str::from_utf8(&data[start..*cur])?;
    Ok(s.parse()
        .map_err(|e| format!("Bad u32 at offset {start}: {e}"))?)
}

/// Parse an ASCII decimal unsigned 64-bit integer at `*cur`, advancing past it.
fn parse_ascii_u64(data: &[u8], cur: &mut usize) -> Result<u64, Box<dyn Error>> {
    let start = *cur;
    while *cur < data.len() && data[*cur].is_ascii_digit() {
        *cur += 1;
    }
    if *cur == start {
        return Err(format!("Expected decimal integer at offset {start}").into());
    }
    let s = std::str::from_utf8(&data[start..*cur])?;
    Ok(s.parse()
        .map_err(|e| format!("Bad u64 at offset {start}: {e}"))?)
}

/// Parse exactly `width` ASCII decimal digits as a u64 (for fixed-width xref fields).
fn parse_ascii_u64_fixed(
    data: &[u8],
    cur: &mut usize,
    width: usize,
) -> Result<u64, Box<dyn Error>> {
    if *cur + width > data.len() {
        return Err(format!("Truncated fixed-width integer field at offset {cur}").into());
    }
    let s = std::str::from_utf8(&data[*cur..*cur + width])?;
    let val: u64 = s
        .trim()
        .parse()
        .map_err(|e| format!("Bad fixed u64 '{s}': {e}"))?;
    *cur += width;
    Ok(val)
}

/// Parse exactly `width` ASCII decimal digits as a u32 (for fixed-width xref fields).
fn parse_ascii_u32_fixed(
    data: &[u8],
    cur: &mut usize,
    width: usize,
) -> Result<u32, Box<dyn Error>> {
    if *cur + width > data.len() {
        return Err(format!("Truncated fixed-width integer field at offset {cur}").into());
    }
    let s = std::str::from_utf8(&data[*cur..*cur + width])?;
    let val: u32 = s
        .trim()
        .parse()
        .map_err(|e| format!("Bad fixed u32 '{s}': {e}"))?;
    *cur += width;
    Ok(val)
}

/// Read `bytes.len()` bytes as a big-endian unsigned integer.
fn read_be_uint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0u64, |acc, &b| (acc << 8) | b as u64)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_traditional_xref_pdf() -> Vec<u8> {
        let mut pdf = Vec::new();
        write!(pdf, "%PDF-1.4\n").unwrap();
        let obj1_off = pdf.len();
        write!(pdf, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n").unwrap();
        let obj2_off = pdf.len();
        write!(
            pdf,
            "2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n"
        )
        .unwrap();
        let xref_off = pdf.len();
        write!(pdf, "xref\n0 3\n").unwrap();
        write!(pdf, "0000000000 65535 f \n").unwrap();
        write!(pdf, "{:010} 00000 n \n", obj1_off).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj2_off).unwrap();
        write!(pdf, "trailer\n<< /Size 3 /Root 1 0 R >>\n").unwrap();
        write!(pdf, "startxref\n{xref_off}\n%%EOF\n").unwrap();
        pdf
    }

    #[test]
    fn parse_traditional_xref() {
        let pdf = make_traditional_xref_pdf();
        let xref = parse_xref(&pdf).unwrap();
        assert_eq!(xref.entries.get(&0), Some(&XrefEntry::Free));
        match xref.entries.get(&1) {
            Some(XrefEntry::InUse { generation: 0, .. }) => {}
            other => panic!("Expected InUse gen 0, got {other:?}"),
        }
        match xref.entries.get(&2) {
            Some(XrefEntry::InUse { generation: 0, .. }) => {}
            other => panic!("Expected InUse gen 0, got {other:?}"),
        }
        assert!(xref.trailer.get(b"Root").is_some());
    }

    #[test]
    fn parse_traditional_xref_multiple_subsections() {
        let mut pdf = Vec::new();
        write!(pdf, "%PDF-1.4\n").unwrap();
        let obj1_off = pdf.len();
        write!(pdf, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n").unwrap();
        let obj2_off = pdf.len();
        write!(
            pdf,
            "2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n"
        )
        .unwrap();
        let obj5_off = pdf.len();
        write!(pdf, "5 0 obj\n<< /Type /Page >>\nendobj\n").unwrap();
        let xref_off = pdf.len();
        // Two subsections: 0-2 and 5-5
        write!(pdf, "xref\n0 3\n").unwrap();
        write!(pdf, "0000000000 65535 f \n").unwrap();
        write!(pdf, "{:010} 00000 n \n", obj1_off).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj2_off).unwrap();
        write!(pdf, "5 1\n").unwrap();
        write!(pdf, "{:010} 00000 n \n", obj5_off).unwrap();
        write!(pdf, "trailer\n<< /Size 6 /Root 1 0 R >>\n").unwrap();
        write!(pdf, "startxref\n{xref_off}\n%%EOF\n").unwrap();

        let xref = parse_xref(&pdf).unwrap();
        assert!(xref.entries.contains_key(&1));
        assert!(xref.entries.contains_key(&2));
        assert!(xref.entries.contains_key(&5));
        assert!(!xref.entries.contains_key(&3));
    }

    #[test]
    fn parse_xref_with_prev() {
        // Two xref sections linked via /Prev
        let mut pdf = Vec::new();
        write!(pdf, "%PDF-1.4\n").unwrap();
        let obj1_off = pdf.len();
        write!(pdf, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n").unwrap();
        let obj2_off = pdf.len();
        write!(
            pdf,
            "2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n"
        )
        .unwrap();
        // First xref section (older)
        let xref1_off = pdf.len();
        write!(pdf, "xref\n0 3\n").unwrap();
        write!(pdf, "0000000000 65535 f \n").unwrap();
        write!(pdf, "{:010} 00000 n \n", obj1_off).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj2_off).unwrap();
        write!(pdf, "trailer\n<< /Size 3 /Root 1 0 R >>\n").unwrap();
        // Second xref section (newer, adds obj 3)
        let obj3_off = pdf.len();
        write!(pdf, "3 0 obj\n<< /Type /Page >>\nendobj\n").unwrap();
        let xref2_off = pdf.len();
        write!(pdf, "xref\n3 1\n").unwrap();
        write!(pdf, "{:010} 00000 n \n", obj3_off).unwrap();
        write!(
            pdf,
            "trailer\n<< /Size 4 /Root 1 0 R /Prev {xref1_off} >>\n"
        )
        .unwrap();
        write!(pdf, "startxref\n{xref2_off}\n%%EOF\n").unwrap();

        let xref = parse_xref(&pdf).unwrap();
        assert!(xref.entries.contains_key(&1));
        assert!(xref.entries.contains_key(&2));
        assert!(xref.entries.contains_key(&3));
    }

    #[test]
    fn invalid_pdf_no_startxref() {
        let result = parse_xref(b"%PDF-1.4\nsome content but no xref");
        assert!(result.is_err());
    }
}
