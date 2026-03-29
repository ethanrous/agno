//! ToUnicode CMap parser for PDF fonts (ISO 32000-1 S9.10.3).
//!
//! Parses `beginbfchar` and `beginbfrange` mappings from a CMap stream
//! to produce a character code -> Unicode mapping table.

use std::collections::HashMap;

/// Parsed ToUnicode mapping: character code -> Unicode code point.
#[derive(Debug, Clone, Default)]
pub struct ToUnicodeMap {
    map: HashMap<u32, u32>,
}

impl ToUnicodeMap {
    /// Look up a character code, returning its Unicode code point.
    pub fn get(&self, code: u32) -> Option<char> {
        self.map.get(&code).and_then(|&cp| char::from_u32(cp))
    }

    /// Return true if the map is empty (no mappings parsed).
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Parse a ToUnicode CMap stream into a lookup map.
///
/// Handles `beginbfchar` (single mappings) and `beginbfrange` (range mappings).
/// Hex strings like `<0041>` are parsed to u32 values.
pub fn parse_to_unicode(data: &[u8]) -> ToUnicodeMap {
    let mut map = HashMap::new();
    let mut pos = 0;

    while pos < data.len() {
        if starts_with_at(data, pos, b"beginbfchar") {
            pos += b"beginbfchar".len();
            pos = parse_bfchar_section(data, pos, &mut map);
        } else if starts_with_at(data, pos, b"beginbfrange") {
            pos += b"beginbfrange".len();
            pos = parse_bfrange_section(data, pos, &mut map);
        } else {
            pos += 1;
        }
    }

    ToUnicodeMap { map }
}

fn starts_with_at(data: &[u8], pos: usize, needle: &[u8]) -> bool {
    pos + needle.len() <= data.len() && &data[pos..pos + needle.len()] == needle
}

/// Parse entries between beginbfchar ... endbfchar.
/// Each entry: `<src_code> <dst_unicode>`
fn parse_bfchar_section(data: &[u8], start: usize, map: &mut HashMap<u32, u32>) -> usize {
    let mut pos = start;
    while pos < data.len() {
        skip_whitespace(data, &mut pos);
        if starts_with_at(data, pos, b"endbfchar") {
            return pos + b"endbfchar".len();
        }
        let src = match parse_hex_string(data, &mut pos) {
            Some(v) => v,
            None => return pos,
        };
        skip_whitespace(data, &mut pos);
        let dst = match parse_hex_string(data, &mut pos) {
            Some(v) => v,
            None => return pos,
        };
        map.insert(src, dst);
    }
    pos
}

/// Parse entries between beginbfrange ... endbfrange.
/// Each entry: `<start> <end> <dst_start>` or `<start> <end> [<d1> <d2> ...]`
fn parse_bfrange_section(data: &[u8], start: usize, map: &mut HashMap<u32, u32>) -> usize {
    const MAX_RANGE_SPAN: u32 = 65536;
    let mut pos = start;
    while pos < data.len() {
        skip_whitespace(data, &mut pos);
        if starts_with_at(data, pos, b"endbfrange") {
            return pos + b"endbfrange".len();
        }
        let range_start = match parse_hex_string(data, &mut pos) {
            Some(v) => v,
            None => return pos,
        };
        skip_whitespace(data, &mut pos);
        let range_end = match parse_hex_string(data, &mut pos) {
            Some(v) => v,
            None => return pos,
        };
        skip_whitespace(data, &mut pos);

        if range_end.saturating_sub(range_start) > MAX_RANGE_SPAN {
            continue;
        }

        if pos < data.len() && data[pos] == b'[' {
            pos += 1;
            for code in range_start..=range_end {
                skip_whitespace(data, &mut pos);
                if pos < data.len() && data[pos] == b']' {
                    break;
                }
                if let Some(dst) = parse_hex_string(data, &mut pos) {
                    map.insert(code, dst);
                }
            }
            skip_whitespace(data, &mut pos);
            if pos < data.len() && data[pos] == b']' {
                pos += 1;
            }
        } else {
            let dst_start = match parse_hex_string(data, &mut pos) {
                Some(v) => v,
                None => return pos,
            };
            for (i, code) in (range_start..=range_end).enumerate() {
                map.insert(code, dst_start + i as u32);
            }
        }
    }
    pos
}

/// Parse a `<XXXX>` hex string and return its value as a u32.
fn parse_hex_string(data: &[u8], pos: &mut usize) -> Option<u32> {
    skip_whitespace(data, pos);
    if *pos >= data.len() || data[*pos] != b'<' {
        return None;
    }
    *pos += 1;
    let start = *pos;
    while *pos < data.len() && data[*pos] != b'>' {
        *pos += 1;
    }
    let hex_bytes = &data[start..*pos];
    if *pos < data.len() {
        *pos += 1;
    }
    let hex_str = std::str::from_utf8(hex_bytes).ok()?;
    u32::from_str_radix(hex_str.trim(), 16).ok()
}

fn skip_whitespace(data: &[u8], pos: &mut usize) {
    while *pos < data.len() && (data[*pos].is_ascii_whitespace() || data[*pos] == b'%') {
        if data[*pos] == b'%' {
            while *pos < data.len() && data[*pos] != b'\n' && data[*pos] != b'\r' {
                *pos += 1;
            }
        }
        *pos += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bfchar() {
        let cmap = b"/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CMapType 2 def
1 begincodespacerange
<00> <FF>
endcodespacerange
3 beginbfchar
<41> <0048>
<42> <0065>
<43> <006C>
endbfchar
endcmap";
        let map = parse_to_unicode(cmap);
        assert_eq!(map.get(0x41), Some('H'));
        assert_eq!(map.get(0x42), Some('e'));
        assert_eq!(map.get(0x43), Some('l'));
        assert_eq!(map.get(0x44), None);
    }

    #[test]
    fn parse_bfrange() {
        let cmap = b"1 begincodespacerange
<00> <FF>
endcodespacerange
1 beginbfrange
<41> <43> <0041>
endbfrange";
        let map = parse_to_unicode(cmap);
        assert_eq!(map.get(0x41), Some('A'));
        assert_eq!(map.get(0x42), Some('B'));
        assert_eq!(map.get(0x43), Some('C'));
        assert_eq!(map.get(0x44), None);
    }

    #[test]
    fn parse_bfrange_with_array() {
        let cmap = b"1 begincodespacerange
<0000> <FFFF>
endcodespacerange
1 beginbfrange
<0100> <0102> [<0041> <0042> <0043>]
endbfrange";
        let map = parse_to_unicode(cmap);
        assert_eq!(map.get(0x0100), Some('A'));
        assert_eq!(map.get(0x0101), Some('B'));
        assert_eq!(map.get(0x0102), Some('C'));
    }

    #[test]
    fn parse_two_byte_codes() {
        let cmap = b"1 begincodespacerange
<0000> <FFFF>
endcodespacerange
1 beginbfchar
<0048> <0048>
endbfchar";
        let map = parse_to_unicode(cmap);
        assert_eq!(map.get(0x0048), Some('H'));
    }

    #[test]
    fn empty_cmap() {
        let cmap = b"begincmap endcmap";
        let map = parse_to_unicode(cmap);
        assert!(map.is_empty());
    }
}
