//! Font resolution and character width metrics for PDF rendering.
//!
//! Provides Standard 14 font metrics (Helvetica, Courier, and variants),
//! font resolution from PDF font dictionaries, and character width lookup.

use std::collections::HashMap;
use std::error::Error;

/// Resolved font data for rendering.
pub enum ResolvedFont {
    Standard14 {
        name: String,
        widths: Vec<f64>,
        encoding: Encoding,
    },
    Embedded {
        data: Vec<u8>,
        encoding: Encoding,
        first_char: u32,
        widths: Vec<f64>,
    },
}

/// PDF character encoding.
#[derive(Debug, Clone)]
pub enum Encoding {
    Named(String),
    Differences {
        base: String,
        diffs: HashMap<u8, String>,
    },
    Identity,
}

/// Get width of a character code in 1/1000 text space units.
pub fn char_width(font: &ResolvedFont, code: u8) -> f64 {
    match font {
        ResolvedFont::Standard14 { widths, .. } => {
            widths.get(code as usize).copied().unwrap_or(0.0)
        }
        ResolvedFont::Embedded {
            widths, first_char, ..
        } => {
            let idx = code as u32;
            if idx >= *first_char && (idx - first_char) < widths.len() as u32 {
                widths[(idx - first_char) as usize]
            } else {
                0.0
            }
        }
    }
}

/// Get Standard 14 font widths (256-entry array, 1/1000 units).
///
/// Courier and its variants are monospaced at 600 units. All other fonts
/// (Helvetica, Times-Roman, Symbol, ZapfDingbats, and unknowns) fall back to
/// Helvetica metrics.
pub fn standard14_widths(name: &str) -> Vec<f64> {
    match name {
        "Courier" | "Courier-Bold" | "Courier-Oblique" | "Courier-BoldOblique" => {
            vec![600.0; 256]
        }
        _ => helvetica_widths(),
    }
}

/// Resolve a font from a PDF font dictionary.
pub fn resolve_font(
    font_dict: &super::objects::PdfObject,
    doc: &super::document::PdfDocument,
) -> Result<ResolvedFont, Box<dyn Error>> {
    let base_font = font_dict
        .get_name_str(b"BaseFont")
        .unwrap_or("Helvetica")
        .to_string();

    // Parse encoding
    let encoding = parse_encoding(font_dict);

    // Check for embedded font via FontDescriptor
    if let Some(descriptor_ref) = font_dict.get(b"FontDescriptor") {
        let descriptor = doc.resolve_value(descriptor_ref)?;
        for key in &[b"FontFile2".as_slice(), b"FontFile3".as_slice()] {
            if let Some(font_file_ref) = descriptor.get(key) {
                let font_obj = doc.resolve_value(font_file_ref)?;
                if let Some((_, data)) = font_obj.as_stream() {
                    let first_char = font_dict.get_i64(b"FirstChar").unwrap_or(0) as u32;
                    let widths = extract_widths(font_dict);
                    return Ok(ResolvedFont::Embedded {
                        data: data.to_vec(),
                        encoding,
                        first_char,
                        widths,
                    });
                }
            }
        }
    }

    // Standard 14 font (or explicit Widths override)
    let widths = if font_dict.get(b"Widths").is_some() {
        extract_widths_as_256(font_dict, &base_font)
    } else {
        standard14_widths(&base_font)
    };

    Ok(ResolvedFont::Standard14 {
        name: base_font,
        widths,
        encoding,
    })
}

fn parse_encoding(font_dict: &super::objects::PdfObject) -> Encoding {
    match font_dict.get(b"Encoding") {
        Some(enc) => {
            if let Some(name) = enc.as_name_str() {
                Encoding::Named(name.to_string())
            } else if enc.as_dict().is_some() {
                let base = enc
                    .get_name_str(b"BaseEncoding")
                    .unwrap_or("WinAnsiEncoding")
                    .to_string();
                let diffs = parse_differences(enc);
                Encoding::Differences { base, diffs }
            } else {
                Encoding::Named("WinAnsiEncoding".to_string())
            }
        }
        None => Encoding::Named("WinAnsiEncoding".to_string()),
    }
}

fn parse_differences(enc: &super::objects::PdfObject) -> HashMap<u8, String> {
    let mut diffs = HashMap::new();
    let arr = match enc.get(b"Differences").and_then(|d| d.as_array()) {
        Some(a) => a,
        None => return diffs,
    };
    let mut current_code: u8 = 0;
    for item in arr {
        if let Some(n) = item.as_i64() {
            current_code = n as u8;
        } else if let Some(name) = item.as_name_str() {
            diffs.insert(current_code, name.to_string());
            current_code = current_code.wrapping_add(1);
        }
    }
    diffs
}

fn extract_widths(font_dict: &super::objects::PdfObject) -> Vec<f64> {
    font_dict
        .get(b"Widths")
        .and_then(|w| w.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect())
        .unwrap_or_default()
}

fn extract_widths_as_256(
    font_dict: &super::objects::PdfObject,
    fallback_name: &str,
) -> Vec<f64> {
    let first_char = font_dict.get_i64(b"FirstChar").unwrap_or(0) as usize;
    let explicit = extract_widths(font_dict);
    let mut widths = standard14_widths(fallback_name);
    for (i, &w) in explicit.iter().enumerate() {
        let idx = first_char + i;
        if idx < 256 {
            widths[idx] = w;
        }
    }
    widths
}

fn helvetica_widths() -> Vec<f64> {
    let mut w = vec![0.0; 256];
    w[32] = 278.0; // space
    w[33] = 278.0; // !
    w[34] = 355.0; // "
    w[35] = 556.0; // #
    w[36] = 556.0; // $
    w[37] = 889.0; // %
    w[38] = 667.0; // &
    w[39] = 191.0; // '
    w[40] = 333.0; // (
    w[41] = 333.0; // )
    w[42] = 389.0; // *
    w[43] = 584.0; // +
    w[44] = 278.0; // ,
    w[45] = 333.0; // -
    w[46] = 278.0; // .
    w[47] = 278.0; // /
    w[48] = 556.0; // 0
    w[49] = 556.0; // 1
    w[50] = 556.0; // 2
    w[51] = 556.0; // 3
    w[52] = 556.0; // 4
    w[53] = 556.0; // 5
    w[54] = 556.0; // 6
    w[55] = 556.0; // 7
    w[56] = 556.0; // 8
    w[57] = 556.0; // 9
    w[58] = 278.0; // :
    w[59] = 278.0; // ;
    w[60] = 584.0; // <
    w[61] = 584.0; // =
    w[62] = 584.0; // >
    w[63] = 556.0; // ?
    w[64] = 1015.0; // @
    w[65] = 667.0; // A
    w[66] = 667.0; // B
    w[67] = 722.0; // C
    w[68] = 722.0; // D
    w[69] = 667.0; // E
    w[70] = 611.0; // F
    w[71] = 778.0; // G
    w[72] = 722.0; // H
    w[73] = 278.0; // I
    w[74] = 500.0; // J
    w[75] = 667.0; // K
    w[76] = 556.0; // L
    w[77] = 833.0; // M
    w[78] = 722.0; // N
    w[79] = 778.0; // O
    w[80] = 667.0; // P
    w[81] = 778.0; // Q
    w[82] = 722.0; // R
    w[83] = 667.0; // S
    w[84] = 611.0; // T
    w[85] = 722.0; // U
    w[86] = 667.0; // V
    w[87] = 944.0; // W
    w[88] = 667.0; // X
    w[89] = 667.0; // Y
    w[90] = 611.0; // Z
    w[91] = 278.0; // [
    w[92] = 278.0; // backslash
    w[93] = 278.0; // ]
    w[94] = 469.0; // ^
    w[95] = 556.0; // _
    w[96] = 333.0; // `
    w[97] = 556.0; // a
    w[98] = 556.0; // b
    w[99] = 500.0; // c
    w[100] = 556.0; // d
    w[101] = 556.0; // e
    w[102] = 278.0; // f
    w[103] = 556.0; // g
    w[104] = 556.0; // h
    w[105] = 222.0; // i
    w[106] = 222.0; // j
    w[107] = 500.0; // k
    w[108] = 222.0; // l
    w[109] = 833.0; // m
    w[110] = 556.0; // n
    w[111] = 556.0; // o
    w[112] = 556.0; // p
    w[113] = 556.0; // q
    w[114] = 333.0; // r
    w[115] = 500.0; // s
    w[116] = 278.0; // t
    w[117] = 556.0; // u
    w[118] = 500.0; // v
    w[119] = 722.0; // w
    w[120] = 500.0; // x
    w[121] = 500.0; // y
    w[122] = 500.0; // z
    w[123] = 334.0; // {
    w[124] = 260.0; // |
    w[125] = 334.0; // }
    w[126] = 584.0; // ~
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard14_helvetica_widths() {
        let widths = standard14_widths("Helvetica");
        assert!((widths[32] - 278.0).abs() < 1.0); // space
        assert!((widths[65] - 667.0).abs() < 1.0); // A
    }

    #[test]
    fn standard14_courier_widths() {
        let widths = standard14_widths("Courier");
        assert!((widths[65] - 600.0).abs() < 1.0);
        assert!((widths[97] - 600.0).abs() < 1.0);
    }

    #[test]
    fn char_width_standard14() {
        let font = ResolvedFont::Standard14 {
            name: "Helvetica".into(),
            widths: standard14_widths("Helvetica"),
            encoding: Encoding::Named("WinAnsiEncoding".into()),
        };
        let w = char_width(&font, b'A');
        assert!((w - 667.0).abs() < 1.0);
    }

    #[test]
    fn char_width_embedded() {
        let font = ResolvedFont::Embedded {
            data: vec![],
            encoding: Encoding::Identity,
            first_char: 32,
            widths: vec![250.0, 300.0, 500.0],
        };
        assert!((char_width(&font, 32) - 250.0).abs() < 1.0);
        assert!((char_width(&font, 33) - 300.0).abs() < 1.0);
        assert!((char_width(&font, 34) - 500.0).abs() < 1.0);
        assert!((char_width(&font, 31) - 0.0).abs() < 1.0); // below first_char
    }

    #[test]
    fn unknown_font_falls_back() {
        let widths = standard14_widths("UnknownFont");
        assert!((widths[65] - 667.0).abs() < 1.0); // Falls back to Helvetica
    }
}
