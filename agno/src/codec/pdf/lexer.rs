//! PDF byte-stream tokenizer (ISO 32000-1 section 7.3).
//!
//! Tokenizes PDF objects: integers, reals, booleans, null, names, literal strings,
//! hex strings, arrays, dictionaries, and indirect references.

use std::collections::HashMap;
use std::error::Error;

use super::objects::{ObjRef, PdfObject};

/// PDF whitespace bytes: NUL, TAB, LF, FF, CR, SPACE (ISO 32000-1 Table 1).
#[inline]
fn is_pdf_whitespace(b: u8) -> bool {
    matches!(b, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// PDF delimiter bytes: `( ) < > [ ] { } / %` plus whitespace.
#[inline]
fn is_delimiter(b: u8) -> bool {
    is_pdf_whitespace(b) || matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

/// A streaming tokenizer over a PDF byte slice.
pub struct Lexer<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Lexer { data, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn set_position(&mut self, pos: usize) {
        self.pos = pos;
    }

    pub fn remaining(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub fn peek_byte(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// Advance past the current byte and return it.
    fn consume_byte(&mut self) -> Option<u8> {
        let b = self.data.get(self.pos).copied()?;
        self.pos += 1;
        Some(b)
    }

    /// Skip all PDF whitespace AND comments (`%` to end-of-line).
    pub fn skip_whitespace(&mut self) {
        loop {
            // Skip whitespace bytes.
            while self.pos < self.data.len() && is_pdf_whitespace(self.data[self.pos]) {
                self.pos += 1;
            }
            // Skip comment: % to end of line.
            if self.pos < self.data.len() && self.data[self.pos] == b'%' {
                self.pos += 1;
                while self.pos < self.data.len() && self.data[self.pos] != b'\n' && self.data[self.pos] != b'\r' {
                    self.pos += 1;
                }
                // Continue outer loop to skip any whitespace/comments that follow.
            } else {
                break;
            }
        }
    }

    /// Read a keyword token (sequence of non-delimiter bytes).
    pub fn read_keyword(&mut self) -> Result<Vec<u8>, Box<dyn Error>> {
        self.skip_whitespace();
        if self.at_end() {
            return Err("Unexpected end of data reading keyword".into());
        }
        let start = self.pos;
        while self.pos < self.data.len() && !is_delimiter(self.data[self.pos]) {
            self.pos += 1;
        }
        if self.pos == start {
            return Err(format!("Expected keyword, found delimiter byte 0x{:02X}", self.data[self.pos]).into());
        }
        Ok(self.data[start..self.pos].to_vec())
    }

    /// Read a keyword and assert it equals `expected`.
    pub fn expect_keyword(&mut self, expected: &[u8]) -> Result<(), Box<dyn Error>> {
        let kw = self.read_keyword()?;
        if kw != expected {
            return Err(format!(
                "Expected keyword {:?}, found {:?}",
                std::str::from_utf8(expected).unwrap_or("<binary>"),
                std::str::from_utf8(&kw).unwrap_or("<binary>"),
            )
            .into());
        }
        Ok(())
    }

    /// Parse and return the next PDF object, or `None` at end of input.
    pub fn next_object(&mut self) -> Result<Option<PdfObject>, Box<dyn Error>> {
        self.skip_whitespace();
        if self.at_end() {
            return Ok(None);
        }

        let b = self.data[self.pos];

        match b {
            // Name object: /identifier
            b'/' => {
                self.pos += 1; // consume '/'
                Ok(Some(self.read_name()?))
            }

            // Literal string: (...)
            b'(' => {
                self.pos += 1; // consume '('
                Ok(Some(self.read_literal_string()?))
            }

            // Either hex string <...> or dictionary <<...>>
            b'<' => {
                if self.data.get(self.pos + 1) == Some(&b'<') {
                    self.pos += 2; // consume '<<'
                    Ok(Some(self.read_dictionary()?))
                } else {
                    self.pos += 1; // consume '<'
                    Ok(Some(self.read_hex_string()?))
                }
            }

            // Array: [...]
            b'[' => {
                self.pos += 1; // consume '['
                Ok(Some(self.read_array()?))
            }

            // End-of-dictionary marker '>>' — caller's read_dictionary loop handles this.
            // If we see it at the top level, it's a parse error.
            b'>' => {
                if self.data.get(self.pos + 1) == Some(&b'>') {
                    // This should have been consumed by read_dictionary; surface as None
                    // so that the dictionary reader can detect EOF-of-dict.
                    return Ok(None);
                }
                Err(format!("Unexpected byte '>' at position {}", self.pos).into())
            }

            // End-of-array marker — same treatment.
            b']' => Ok(None),

            // Boolean true
            b't' if self.data[self.pos..].starts_with(b"true") => {
                self.pos += 4;
                Ok(Some(PdfObject::Bool(true)))
            }

            // Boolean false
            b'f' if self.data[self.pos..].starts_with(b"false") => {
                self.pos += 5;
                Ok(Some(PdfObject::Bool(false)))
            }

            // Null
            b'n' if self.data[self.pos..].starts_with(b"null") => {
                self.pos += 4;
                Ok(Some(PdfObject::Null))
            }

            // Number (integer or real), possibly followed by `N G R` indirect reference.
            b'0'..=b'9' | b'+' | b'-' | b'.' => Ok(Some(self.read_number_or_reference()?)),

            other => Err(format!("Unexpected byte 0x{other:02X} ('{}') at position {}", other as char, self.pos).into()),
        }
    }

    // -------------------------------------------------------------------------
    // Private helpers
    // -------------------------------------------------------------------------

    /// Read a PDF Name after the leading `/` has been consumed.
    fn read_name(&mut self) -> Result<PdfObject, Box<dyn Error>> {
        let mut name = Vec::new();
        while self.pos < self.data.len() && !is_delimiter(self.data[self.pos]) {
            let b = self.data[self.pos];
            if b == b'#' {
                // Hex escape #XX
                self.pos += 1;
                let hi = self
                    .data
                    .get(self.pos)
                    .copied()
                    .ok_or("Truncated hex escape in name")?;
                let lo = self
                    .data
                    .get(self.pos + 1)
                    .copied()
                    .ok_or("Truncated hex escape in name")?;
                let decoded = decode_hex_byte(hi, lo)
                    .ok_or_else(|| format!("Invalid hex escape #{hi:02X}{lo:02X} in name"))?;
                name.push(decoded);
                self.pos += 2;
            } else {
                name.push(b);
                self.pos += 1;
            }
        }
        Ok(PdfObject::Name(name))
    }

    /// Read a literal string after the leading `(` has been consumed.
    /// Handles `\n`, `\r`, `\\`, `\(`, `\)`, octal `\NNN`, and nested parens.
    fn read_literal_string(&mut self) -> Result<PdfObject, Box<dyn Error>> {
        let mut buf = Vec::new();
        let mut depth: usize = 1; // we've already consumed the opening '('

        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            match b {
                b'(' => {
                    depth += 1;
                    buf.push(b'(');
                    self.pos += 1;
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        self.pos += 1; // consume closing ')'
                        return Ok(PdfObject::String(buf));
                    }
                    buf.push(b')');
                    self.pos += 1;
                }
                b'\\' => {
                    self.pos += 1; // consume '\'
                    match self.data.get(self.pos).copied() {
                        Some(b'n') => { buf.push(b'\n'); self.pos += 1; }
                        Some(b'r') => { buf.push(b'\r'); self.pos += 1; }
                        Some(b't') => { buf.push(b'\t'); self.pos += 1; }
                        Some(b'b') => { buf.push(0x08); self.pos += 1; }
                        Some(b'f') => { buf.push(0x0C); self.pos += 1; }
                        Some(b'(') => { buf.push(b'('); self.pos += 1; }
                        Some(b')') => { buf.push(b')'); self.pos += 1; }
                        Some(b'\\') => { buf.push(b'\\'); self.pos += 1; }
                        // Line continuation: backslash followed by newline — skip both.
                        Some(b'\r') => {
                            self.pos += 1;
                            if self.data.get(self.pos).copied() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        Some(b'\n') => { self.pos += 1; }
                        // Octal escape: \NNN (1-3 octal digits).
                        Some(d) if d.is_ascii_digit() && d <= b'7' => {
                            let mut val: u8 = d - b'0';
                            self.pos += 1;
                            for _ in 0..2 {
                                match self.data.get(self.pos).copied() {
                                    Some(d2) if d2.is_ascii_digit() && d2 <= b'7' => {
                                        val = val.wrapping_mul(8).wrapping_add(d2 - b'0');
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            buf.push(val);
                        }
                        // Unknown escape: treat as literal next byte.
                        Some(other) => { buf.push(other); self.pos += 1; }
                        None => return Err("Truncated escape sequence in literal string".into()),
                    }
                }
                _ => {
                    buf.push(b);
                    self.pos += 1;
                }
            }
        }
        Err("Unterminated literal string".into())
    }

    /// Read a hex string after the leading `<` has been consumed.
    fn read_hex_string(&mut self) -> Result<PdfObject, Box<dyn Error>> {
        let mut nibbles: Vec<u8> = Vec::new();
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            if b == b'>' {
                self.pos += 1; // consume '>'
                // Convert nibble pairs to bytes; odd trailing nibble treated as if padded with 0.
                let mut bytes = Vec::with_capacity((nibbles.len() + 1) / 2);
                let mut i = 0;
                while i < nibbles.len() {
                    let hi = nibbles[i];
                    let lo = *nibbles.get(i + 1).unwrap_or(&0);
                    bytes.push((hi << 4) | lo);
                    i += 2;
                }
                return Ok(PdfObject::HexString(bytes));
            }
            if is_pdf_whitespace(b) {
                self.pos += 1;
                continue;
            }
            let nib = hex_nibble(b)
                .ok_or_else(|| format!("Invalid hex character 0x{b:02X} in hex string"))?;
            nibbles.push(nib);
            self.pos += 1;
        }
        Err("Unterminated hex string".into())
    }

    /// Read an array after the leading `[` has been consumed.
    fn read_array(&mut self) -> Result<PdfObject, Box<dyn Error>> {
        let mut items: Vec<PdfObject> = Vec::new();
        loop {
            self.skip_whitespace();
            if self.at_end() {
                return Err("Unterminated array".into());
            }
            if self.data[self.pos] == b']' {
                self.pos += 1; // consume ']'
                return Ok(PdfObject::Array(items));
            }
            match self.next_object()? {
                Some(obj) => items.push(obj),
                None => {
                    // None is returned when we see `]` — consume it and return.
                    if !self.at_end() && self.data[self.pos] == b']' {
                        self.pos += 1;
                    }
                    return Ok(PdfObject::Array(items));
                }
            }
        }
    }

    /// Read a dictionary after `<<` has been consumed.
    fn read_dictionary(&mut self) -> Result<PdfObject, Box<dyn Error>> {
        let mut dict: HashMap<Vec<u8>, PdfObject> = HashMap::new();
        loop {
            self.skip_whitespace();
            if self.at_end() {
                return Err("Unterminated dictionary".into());
            }
            // Check for end-of-dictionary `>>`
            if self.data.get(self.pos) == Some(&b'>') && self.data.get(self.pos + 1) == Some(&b'>') {
                self.pos += 2;
                return Ok(PdfObject::Dictionary(dict));
            }
            // Next token must be a Name (key).
            let key = match self.next_object()? {
                Some(PdfObject::Name(n)) => n,
                Some(other) => {
                    return Err(format!("Dictionary key must be a Name, got: {other}").into())
                }
                None => return Err("Unexpected end in dictionary (expected key)".into()),
            };
            // Value.
            let val = self
                .next_object()?
                .ok_or("Unexpected end in dictionary (expected value)")?;
            dict.insert(key, val);
        }
    }

    /// Parse a number, then look ahead for `N G R` indirect-reference syntax.
    fn read_number_or_reference(&mut self) -> Result<PdfObject, Box<dyn Error>> {
        let first_start = self.pos;
        let first = self.parse_number_token()?;

        // Only integer objects can be the object-number in a reference.
        if let PdfObject::Integer(num) = first {
            let saved = self.pos;
            self.skip_whitespace();
            // Try to read a second integer (generation number).
            match self.try_parse_integer() {
                Some(gen_val) if gen_val >= 0 => {
                    let saved2 = self.pos;
                    self.skip_whitespace();
                    if self.data.get(self.pos) == Some(&b'R')
                        && (self.pos + 1 >= self.data.len() || is_delimiter(self.data[self.pos + 1]))
                    {
                        self.pos += 1; // consume 'R'
                        return Ok(PdfObject::Reference(ObjRef {
                            num: num as u32,
                            gen_num: gen_val as u16,
                        }));
                    }
                    // Not a reference — restore to after first number.
                    let _ = (saved2, first_start);
                    self.pos = saved;
                }
                _ => {
                    self.pos = saved;
                }
            }
        }

        Ok(first)
    }

    /// Parse a numeric token (integer or real) starting at the current position.
    fn parse_number_token(&mut self) -> Result<PdfObject, Box<dyn Error>> {
        let start = self.pos;
        // Optional sign.
        if self.pos < self.data.len() && matches!(self.data[self.pos], b'+' | b'-') {
            self.pos += 1;
        }
        // Digits before optional decimal point.
        while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        // Optional fractional part.
        let is_real = self.pos < self.data.len() && self.data[self.pos] == b'.';
        if is_real {
            self.pos += 1; // consume '.'
            while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        let token = &self.data[start..self.pos];
        if token.is_empty() || token == b"+" || token == b"-" || token == b"." {
            return Err(format!("Invalid number token at position {start}").into());
        }
        let s = std::str::from_utf8(token)?;
        if is_real || token.contains(&b'.') {
            let f: f64 = s.parse().map_err(|e| format!("Bad real '{s}': {e}"))?;
            Ok(PdfObject::Real(f))
        } else {
            let n: i64 = s.parse().map_err(|e| format!("Bad integer '{s}': {e}"))?;
            Ok(PdfObject::Integer(n))
        }
    }

    /// Try to parse an integer token starting at the current position.
    /// Returns `None` (and does NOT advance `pos`) if no integer is found.
    fn try_parse_integer(&mut self) -> Option<i64> {
        let start = self.pos;
        if self.pos < self.data.len() && matches!(self.data[self.pos], b'+' | b'-') {
            self.pos += 1;
        }
        let digit_start = self.pos;
        while self.pos < self.data.len() && self.data[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos == digit_start {
            self.pos = start;
            return None;
        }
        // Must be followed by a delimiter (or end-of-data) to qualify as a clean integer.
        if self.pos < self.data.len() && !is_delimiter(self.data[self.pos]) {
            self.pos = start;
            return None;
        }
        let s = std::str::from_utf8(&self.data[start..self.pos]).ok()?;
        let n: i64 = s.parse().ok()?;
        Some(n)
    }
}

// -------------------------------------------------------------------------
// Small stateless utilities
// -------------------------------------------------------------------------

/// Convert a single ASCII hex character to its nibble value (0-15).
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Decode two ASCII hex characters into a byte value.
fn decode_hex_byte(hi: u8, lo: u8) -> Option<u8> {
    Some((hex_nibble(hi)? << 4) | hex_nibble(lo)?)
}

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_integer() {
        let mut lex = Lexer::new(b"42");
        assert_eq!(lex.next_object().unwrap().unwrap().as_i64(), Some(42));
    }

    #[test]
    fn parse_negative_integer() {
        let mut lex = Lexer::new(b"-7");
        assert_eq!(lex.next_object().unwrap().unwrap().as_i64(), Some(-7));
    }

    #[test]
    fn parse_real() {
        let mut lex = Lexer::new(b"3.14");
        let obj = lex.next_object().unwrap().unwrap();
        assert!((obj.as_f64().unwrap() - 3.14).abs() < 1e-9);
    }

    #[test]
    fn parse_bool_true() {
        let mut lex = Lexer::new(b"true");
        assert_eq!(lex.next_object().unwrap().unwrap().as_bool(), Some(true));
    }

    #[test]
    fn parse_bool_false() {
        let mut lex = Lexer::new(b"false");
        assert_eq!(lex.next_object().unwrap().unwrap().as_bool(), Some(false));
    }

    #[test]
    fn parse_null() {
        let mut lex = Lexer::new(b"null");
        let obj = lex.next_object().unwrap().unwrap();
        assert!(matches!(obj, PdfObject::Null));
    }

    #[test]
    fn parse_name() {
        let mut lex = Lexer::new(b"/Type");
        let obj = lex.next_object().unwrap().unwrap();
        assert_eq!(obj.as_name_str(), Some("Type"));
    }

    #[test]
    fn parse_name_with_hex_escape() {
        let mut lex = Lexer::new(b"/A#42");
        let obj = lex.next_object().unwrap().unwrap();
        assert_eq!(obj.as_name(), Some(b"AB".as_slice()));
    }

    #[test]
    fn parse_literal_string() {
        let mut lex = Lexer::new(b"(Hello World)");
        let obj = lex.next_object().unwrap().unwrap();
        assert_eq!(obj.as_string_bytes(), Some(b"Hello World".as_slice()));
    }

    #[test]
    fn parse_literal_string_with_escapes() {
        let mut lex = Lexer::new(b"(line1\\nline2)");
        let obj = lex.next_object().unwrap().unwrap();
        assert_eq!(obj.as_string_bytes(), Some(b"line1\nline2".as_slice()));
    }

    #[test]
    fn parse_literal_string_nested_parens() {
        let mut lex = Lexer::new(b"(a(b)c)");
        let obj = lex.next_object().unwrap().unwrap();
        assert_eq!(obj.as_string_bytes(), Some(b"a(b)c".as_slice()));
    }

    #[test]
    fn parse_hex_string() {
        let mut lex = Lexer::new(b"<48656C6C6F>");
        let obj = lex.next_object().unwrap().unwrap();
        assert_eq!(obj.as_string_bytes(), Some(b"Hello".as_slice()));
    }

    #[test]
    fn parse_hex_string_odd_length() {
        let mut lex = Lexer::new(b"<ABC>");
        let obj = lex.next_object().unwrap().unwrap();
        assert_eq!(obj.as_string_bytes(), Some(&[0xAB, 0xC0][..]));
    }

    #[test]
    fn parse_array() {
        let mut lex = Lexer::new(b"[1 2.0 /Name (text)]");
        let obj = lex.next_object().unwrap().unwrap();
        let arr = obj.as_array().unwrap();
        assert_eq!(arr.len(), 4);
        assert_eq!(arr[0].as_i64(), Some(1));
        assert!((arr[1].as_f64().unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(arr[2].as_name_str(), Some("Name"));
        assert_eq!(arr[3].as_string_bytes(), Some(b"text".as_slice()));
    }

    #[test]
    fn parse_dictionary() {
        let mut lex = Lexer::new(b"<< /Type /Page /Count 5 >>");
        let obj = lex.next_object().unwrap().unwrap();
        assert_eq!(obj.get_name_str(b"Type"), Some("Page"));
        assert_eq!(obj.get_i64(b"Count"), Some(5));
    }

    #[test]
    fn parse_indirect_reference() {
        let mut lex = Lexer::new(b"10 0 R");
        let obj = lex.next_object().unwrap().unwrap();
        assert_eq!(obj.as_reference(), Some(ObjRef { num: 10, gen_num: 0 }));
    }

    #[test]
    fn skip_comments() {
        let mut lex = Lexer::new(b"% this is a comment\n42");
        assert_eq!(lex.next_object().unwrap().unwrap().as_i64(), Some(42));
    }

    #[test]
    fn parse_multiple_objects() {
        let mut lex = Lexer::new(b"1 2 3");
        assert_eq!(lex.next_object().unwrap().unwrap().as_i64(), Some(1));
        assert_eq!(lex.next_object().unwrap().unwrap().as_i64(), Some(2));
        assert_eq!(lex.next_object().unwrap().unwrap().as_i64(), Some(3));
        assert!(lex.next_object().unwrap().is_none());
    }

    #[test]
    fn parse_nested_dict_in_array() {
        let mut lex = Lexer::new(b"[<< /A 1 >> << /B 2 >>]");
        let obj = lex.next_object().unwrap().unwrap();
        let arr = obj.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].get_i64(b"A"), Some(1));
        assert_eq!(arr[1].get_i64(b"B"), Some(2));
    }
}
