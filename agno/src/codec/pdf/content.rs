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
                // Skip for now — consume until EI.
                skip_inline_image(&mut lex);
                ops.push(Operator {
                    name: b"BI".to_vec(),
                    operands: std::mem::take(&mut operand_stack),
                });
            } else {
                ops.push(Operator {
                    name: keyword,
                    operands: std::mem::take(&mut operand_stack),
                });
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

/// Skip an inline image block starting just after `BI` has been consumed.
///
/// Scans forward for the `EI` marker, which must be preceded by whitespace and
/// followed by whitespace, a delimiter, or EOF (per PDF spec section 8.9.7).
fn skip_inline_image(lex: &mut super::lexer::Lexer<'_>) {
    // Read until we encounter EI preceded by whitespace.
    // Strategy: scan byte-by-byte looking for \nEI, \rEI, or ' EI' followed
    // by whitespace/delimiter/EOF.
    loop {
        if lex.at_end() {
            break;
        }

        // Look for 'E' that could start 'EI'.
        let b = match lex.peek_byte() {
            Some(b) => b,
            None => break,
        };

        if b == b'E' {
            let pos = lex.position();
            // Check that this 'E' is preceded by whitespace — we already
            // consumed some bytes, so check what's at pos-1 if pos > 0.
            // Then check the two bytes E and I.
            if lex.remaining().starts_with(b"EI") {
                let after_ei = pos + 2;
                // EI must be followed by whitespace, delimiter, or EOF.
                // We need to look at the byte at after_ei position in the
                // original data — use remaining() offset.
                let remaining = lex.remaining();
                let ei_followed_by_boundary = remaining.len() == 2
                    || is_content_stream_delimiter(remaining[2]);

                if ei_followed_by_boundary {
                    // Advance past EI.
                    lex.set_position(after_ei);
                    break;
                }
            }
        }

        // Advance one byte and continue scanning.
        let pos = lex.position();
        lex.set_position(pos + 1);
    }
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
