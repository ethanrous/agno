//! Shared small utilities for the PDF module.

/// Convert a single ASCII hex character to its nibble value (0-15).
#[inline]
pub(super) fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// PDF whitespace bytes: NUL, TAB, LF, FF, CR, SPACE (ISO 32000-1 Table 1).
#[inline]
pub(super) fn is_pdf_whitespace(b: u8) -> bool {
    matches!(b, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}
