//! Font resolution and character width metrics for PDF rendering.
//!
//! Provides Standard 14 font metrics (Helvetica, Courier, and variants),
//! font resolution from PDF font dictionaries, and character width lookup.

use std::collections::HashMap;
use std::error::Error;

use super::cmap::ToUnicodeMap;

/// Resolved font data for rendering.
pub enum ResolvedFont {
    Standard14 {
        name: String,
        widths: Vec<f64>,
        encoding: Encoding,
        to_unicode: Option<ToUnicodeMap>,
    },
    Embedded {
        data: Vec<u8>,
        encoding: Encoding,
        first_char: u32,
        widths: Vec<f64>,
        to_unicode: Option<ToUnicodeMap>,
    },
    CIDFont {
        data: Vec<u8>,
        cid_to_gid: CIDToGIDMap,
        default_width: f64,
        widths: CIDWidths,
        to_unicode: Option<ToUnicodeMap>,
        is_two_byte: bool,
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

/// CID-to-GID mapping strategy.
#[derive(Debug, Clone)]
pub enum CIDToGIDMap {
    Identity,
    Explicit(Vec<u16>),
}

/// CID width table: maps CID ranges to advance widths.
#[derive(Debug, Clone, Default)]
pub struct CIDWidths {
    individual: Vec<(u32, Vec<f64>)>,
    ranges: Vec<(u32, u32, f64)>,
}

impl CIDWidths {
    pub fn get(&self, cid: u32) -> Option<f64> {
        for (start, widths) in &self.individual {
            if cid >= *start {
                let idx = (cid - *start) as usize;
                if idx < widths.len() {
                    return Some(widths[idx]);
                }
            }
        }
        for &(start, end, width) in &self.ranges {
            if cid >= start && cid <= end {
                return Some(width);
            }
        }
        None
    }
}

/// Get width of a character code in 1/1000 text space units.
pub fn char_width(font: &ResolvedFont, code: u8) -> f64 {
    char_width_u32(font, code as u32)
}

/// Get width of a character code (u32 for CIDFont compatibility).
pub fn char_width_u32(font: &ResolvedFont, code: u32) -> f64 {
    match font {
        ResolvedFont::Standard14 { widths, .. } => {
            widths.get(code as usize).copied().unwrap_or(0.0)
        }
        ResolvedFont::Embedded {
            widths, first_char, ..
        } => {
            if code >= *first_char && (code - first_char) < widths.len() as u32 {
                widths[(code - first_char) as usize]
            } else {
                0.0
            }
        }
        ResolvedFont::CIDFont {
            default_width, widths, ..
        } => widths.get(code).unwrap_or(*default_width),
    }
}

/// Get Standard 14 font widths (256-entry array, 1/1000 units).
pub fn standard14_widths(name: &str) -> Vec<f64> {
    match name {
        "Courier" | "Courier-Bold" | "Courier-Oblique" | "Courier-BoldOblique" => {
            vec![600.0; 256]
        }
        _ => helvetica_widths(),
    }
}

/// Return the TTC face index for a Standard 14 font name within a TrueType Collection.
/// Helvetica.ttc: 0=Regular, 1=Bold, 2=Oblique, 3=BoldOblique, 4=Light, 5=LightOblique
/// Courier.ttc:   0=Regular, 1=Bold, 2=Oblique, 3=BoldOblique
/// Times.ttc:     0=Roman, 1=Bold, 2=Italic, 3=BoldItalic
pub fn ttc_face_index(name: &str) -> u32 {
    match name {
        "Helvetica-Bold" | "Courier-Bold" | "Times-Bold" => 1,
        "Helvetica-Oblique" | "Courier-Oblique" | "Times-Italic" => 2,
        "Helvetica-BoldOblique" | "Courier-BoldOblique" | "Times-BoldItalic" => 3,
        _ => 0,
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

    let encoding = parse_encoding(font_dict, doc);
    let to_unicode = parse_to_unicode_map(font_dict, doc);

    let subtype = font_dict.get_name_str(b"Subtype").unwrap_or("");
    if subtype == "Type0" {
        return resolve_type0_font(font_dict, doc);
    }

    // Check for embedded font via FontDescriptor
    if let Some(descriptor_ref) = font_dict.get(b"FontDescriptor") {
        let descriptor = doc.resolve_value(descriptor_ref)?;
        for key in &[b"FontFile2".as_slice(), b"FontFile3".as_slice(), b"FontFile".as_slice()] {
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
                        to_unicode,
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
        to_unicode,
    })
}

fn resolve_type0_font(
    font_dict: &super::objects::PdfObject,
    doc: &super::document::PdfDocument,
) -> Result<ResolvedFont, Box<dyn Error>> {
    let to_unicode = parse_to_unicode_map(font_dict, doc);

    let descendants = font_dict
        .get(b"DescendantFonts")
        .ok_or("Type0 font missing /DescendantFonts")?;
    let descendants = doc.resolve_value(descendants)?;
    let desc_arr = descendants
        .as_array()
        .ok_or("DescendantFonts is not an array")?;
    if desc_arr.is_empty() {
        return Err("DescendantFonts array is empty".into());
    }
    let cid_font = doc.resolve_value(&desc_arr[0])?;

    let encoding_name = font_dict.get_name_str(b"Encoding").unwrap_or("Identity-H");
    let is_two_byte = encoding_name.contains("Identity")
        || encoding_name.ends_with("-H")
        || encoding_name.ends_with("-V");

    let mut font_data = Vec::new();
    if let Some(desc_ref) = cid_font.get(b"FontDescriptor") {
        let descriptor = doc.resolve_value(desc_ref)?;
        for key in &[b"FontFile2".as_slice(), b"FontFile3".as_slice()] {
            if let Some(ff_ref) = descriptor.get(key) {
                let ff_obj = doc.resolve_value(ff_ref)?;
                if let Some((_, data)) = ff_obj.as_stream() {
                    font_data = data.to_vec();
                    break;
                }
            }
        }
    }

    let cid_to_gid = match cid_font.get(b"CIDToGIDMap") {
        Some(obj) => {
            if obj.as_name_str().is_some() {
                CIDToGIDMap::Identity
            } else {
                let resolved = doc.resolve_value(obj)?;
                if let Some((_, data)) = resolved.as_stream() {
                    let gids: Vec<u16> = data
                        .chunks_exact(2)
                        .map(|c| u16::from_be_bytes([c[0], c[1]]))
                        .collect();
                    CIDToGIDMap::Explicit(gids)
                } else {
                    CIDToGIDMap::Identity
                }
            }
        }
        None => CIDToGIDMap::Identity,
    };

    let default_width = cid_font.get_f64(b"DW").unwrap_or(1000.0);
    let widths = parse_cid_widths(&cid_font, doc);

    Ok(ResolvedFont::CIDFont {
        data: font_data,
        cid_to_gid,
        default_width,
        widths,
        to_unicode,
        is_two_byte,
    })
}

fn parse_cid_widths(
    cid_font: &super::objects::PdfObject,
    doc: &super::document::PdfDocument,
) -> CIDWidths {
    let mut cw = CIDWidths::default();
    let w_arr = match cid_font
        .get(b"W")
        .and_then(|w| doc.resolve_value(w).ok())
        .and_then(|w| w.as_array().map(|a| a.to_vec()))
    {
        Some(a) => a,
        None => return cw,
    };

    let mut i = 0;
    while i < w_arr.len() {
        let start = match w_arr[i].as_i64() {
            Some(n) => n as u32,
            None => { i += 1; continue; }
        };
        i += 1;
        if i >= w_arr.len() { break; }
        if let Some(arr) = w_arr[i].as_array() {
            let widths: Vec<f64> = arr.iter().filter_map(|v| v.as_f64()).collect();
            cw.individual.push((start, widths));
            i += 1;
        } else if let Some(end) = w_arr[i].as_i64() {
            i += 1;
            if i < w_arr.len() {
                let width = w_arr[i].as_f64().unwrap_or(1000.0);
                cw.ranges.push((start, end as u32, width));
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    cw
}

fn parse_encoding(
    font_dict: &super::objects::PdfObject,
    doc: &super::document::PdfDocument,
) -> Encoding {
    let enc = match font_dict.get(b"Encoding") {
        Some(e) => e.clone(),
        None => return Encoding::Named("WinAnsiEncoding".to_string()),
    };

    // Resolve indirect reference if needed.
    let enc = if enc.as_name_str().is_some() || enc.as_dict().is_some() {
        enc
    } else {
        match doc.resolve_value(&enc) {
            Ok(resolved) => resolved,
            Err(_) => return Encoding::Named("WinAnsiEncoding".to_string()),
        }
    };

    if let Some(name) = enc.as_name_str() {
        Encoding::Named(name.to_string())
    } else if enc.as_dict().is_some() {
        let base = enc
            .get_name_str(b"BaseEncoding")
            .unwrap_or("WinAnsiEncoding")
            .to_string();
        let diffs = parse_differences(&enc);
        Encoding::Differences { base, diffs }
    } else {
        Encoding::Named("WinAnsiEncoding".to_string())
    }
}

fn parse_to_unicode_map(
    font_dict: &super::objects::PdfObject,
    doc: &super::document::PdfDocument,
) -> Option<ToUnicodeMap> {
    let tu_ref = font_dict.get(b"ToUnicode")?;
    let tu_obj = doc.resolve_value(tu_ref).ok()?;
    let (_, data) = tu_obj.as_stream()?;
    let map = super::cmap::parse_to_unicode(data);
    if map.is_empty() { None } else { Some(map) }
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
    w[32] = 278.0; w[33] = 278.0; w[34] = 355.0; w[35] = 556.0;
    w[36] = 556.0; w[37] = 889.0; w[38] = 667.0; w[39] = 191.0;
    w[40] = 333.0; w[41] = 333.0; w[42] = 389.0; w[43] = 584.0;
    w[44] = 278.0; w[45] = 333.0; w[46] = 278.0; w[47] = 278.0;
    w[48] = 556.0; w[49] = 556.0; w[50] = 556.0; w[51] = 556.0;
    w[52] = 556.0; w[53] = 556.0; w[54] = 556.0; w[55] = 556.0;
    w[56] = 556.0; w[57] = 556.0; w[58] = 278.0; w[59] = 278.0;
    w[60] = 584.0; w[61] = 584.0; w[62] = 584.0; w[63] = 556.0;
    w[64] = 1015.0;
    w[65] = 667.0; w[66] = 667.0; w[67] = 722.0; w[68] = 722.0;
    w[69] = 667.0; w[70] = 611.0; w[71] = 778.0; w[72] = 722.0;
    w[73] = 278.0; w[74] = 500.0; w[75] = 667.0; w[76] = 556.0;
    w[77] = 833.0; w[78] = 722.0; w[79] = 778.0; w[80] = 667.0;
    w[81] = 778.0; w[82] = 722.0; w[83] = 667.0; w[84] = 611.0;
    w[85] = 722.0; w[86] = 667.0; w[87] = 944.0; w[88] = 667.0;
    w[89] = 667.0; w[90] = 611.0;
    w[91] = 278.0; w[92] = 278.0; w[93] = 278.0; w[94] = 469.0;
    w[95] = 556.0; w[96] = 333.0;
    w[97] = 556.0; w[98] = 556.0; w[99] = 500.0; w[100] = 556.0;
    w[101] = 556.0; w[102] = 278.0; w[103] = 556.0; w[104] = 556.0;
    w[105] = 222.0; w[106] = 222.0; w[107] = 500.0; w[108] = 222.0;
    w[109] = 833.0; w[110] = 556.0; w[111] = 556.0; w[112] = 556.0;
    w[113] = 556.0; w[114] = 333.0; w[115] = 500.0; w[116] = 278.0;
    w[117] = 556.0; w[118] = 500.0; w[119] = 722.0; w[120] = 500.0;
    w[121] = 500.0; w[122] = 500.0;
    w[123] = 334.0; w[124] = 260.0; w[125] = 334.0; w[126] = 584.0;
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard14_helvetica_widths() {
        let widths = standard14_widths("Helvetica");
        assert!((widths[32] - 278.0).abs() < 1.0);
        assert!((widths[65] - 667.0).abs() < 1.0);
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
            to_unicode: None,
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
            to_unicode: None,
        };
        assert!((char_width(&font, 32) - 250.0).abs() < 1.0);
        assert!((char_width(&font, 33) - 300.0).abs() < 1.0);
        assert!((char_width(&font, 34) - 500.0).abs() < 1.0);
        assert!((char_width(&font, 31) - 0.0).abs() < 1.0);
    }

    #[test]
    fn unknown_font_falls_back() {
        let widths = standard14_widths("UnknownFont");
        assert!((widths[65] - 667.0).abs() < 1.0);
    }

    #[test]
    fn ttc_face_index_bold() {
        assert_eq!(ttc_face_index("Helvetica-Bold"), 1);
        assert_eq!(ttc_face_index("Helvetica"), 0);
        assert_eq!(ttc_face_index("Helvetica-Oblique"), 2);
        assert_eq!(ttc_face_index("Helvetica-BoldOblique"), 3);
    }
}
