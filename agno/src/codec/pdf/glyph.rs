//! Glyph outline rendering for PDF text.
//!
//! Extracts glyph outlines from embedded and system fonts via `ttf_parser`,
//! converts them to `tiny_skia::Path` objects, and handles the full
//! encoding-aware glyph ID resolution pipeline.

use std::collections::HashMap;

use tiny_skia::{FillRule, Mask, Paint, Path, PathBuilder, Pixmap, Transform};

use super::cmap::ToUnicodeMap;
use super::font::{Encoding, ResolvedFont};
use super::graphics::Color;
use super::system_fonts::system_font_for_standard14;
use super::text::PositionedGlyph;

/// Build a cache of pre-prepared font data (with CFF wrapping done once per font).
/// Eliminates per-glyph `wrap_cff_in_otf` calls.
pub(super) fn build_font_data_cache(
    fonts: &HashMap<Vec<u8>, ResolvedFont>,
) -> HashMap<Vec<u8>, Vec<u8>> {
    let mut cache = HashMap::new();
    for (name, font) in fonts {
        match font {
            ResolvedFont::Embedded { data, .. } | ResolvedFont::CIDFont { data, .. } => {
                if !data.is_empty() {
                    let prepared = match ttf_parser::Face::parse(data, 0) {
                        Ok(_) => data.clone(),
                        Err(_) => wrap_cff_in_otf(data),
                    };
                    cache.insert(name.clone(), prepared);
                }
            }
            ResolvedFont::Standard14 {
                name: font_name, ..
            } => {
                if let Some(sys_data) = system_font_for_standard14(font_name) {
                    cache.insert(name.clone(), sys_data.clone());
                }
            }
        }
    }
    cache
}

/// Render a single positioned glyph onto the pixmap. Uses embedded font outlines
/// via ttf-parser when available, falls back to filled rectangles for Standard 14
/// fonts without system font substitutes.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_glyph(
    glyph: &PositionedGlyph,
    fonts: &HashMap<Vec<u8>, ResolvedFont>,
    font_data_cache: &HashMap<Vec<u8>, Vec<u8>>,
    color: &Color,
    alpha: f64,
    transform: Transform,
    pixmap: &mut Pixmap,
    mask: Option<&Mask>,
) {
    let font_size = glyph.font_size_user_space;
    if font_size < 0.5 {
        return;
    }

    let mut paint = Paint::default();
    paint.set_color_rgba8(
        (color.r.clamp(0.0, 1.0) * 255.0) as u8,
        (color.g.clamp(0.0, 1.0) * 255.0) as u8,
        (color.b.clamp(0.0, 1.0) * 255.0) as u8,
        (alpha.clamp(0.0, 1.0) * 255.0) as u8,
    );
    paint.anti_alias = true;

    // Use cached font data to avoid per-glyph CFF wrapping.
    let cached_data = font_data_cache.get(glyph.font_name);

    // Try font outline rendering with encoding-aware mapping.
    match fonts.get(glyph.font_name) {
        Some(ResolvedFont::Embedded {
            data,
            first_char,
            encoding,
            to_unicode,
            ..
        }) => {
            let font_bytes = cached_data.map(|v| v.as_slice()).unwrap_or(data);
            if let Some(path) = glyph_outline_path_cached(
                font_bytes,
                glyph,
                *first_char,
                encoding,
                to_unicode.as_ref(),
                0,
            ) {
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
                return;
            }
        }
        Some(ResolvedFont::Standard14 {
            name,
            encoding,
            to_unicode,
            ..
        }) => {
            let face_idx = super::font::ttc_face_index(name);
            if let Some(font_bytes) = cached_data
                && let Some(path) = glyph_outline_path_cached(
                    font_bytes,
                    glyph,
                    0,
                    encoding,
                    to_unicode.as_ref(),
                    face_idx,
                )
            {
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
                return;
            } else if let Some(sys_data) = system_font_for_standard14(name)
                && let Some(path) = glyph_outline_path_cached(
                    sys_data,
                    glyph,
                    0,
                    encoding,
                    to_unicode.as_ref(),
                    face_idx,
                )
            {
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
                return;
            }
        }
        Some(ResolvedFont::CIDFont {
            data,
            cid_to_gid,
            to_unicode,
            ..
        }) => {
            let font_bytes = cached_data.map(|v| v.as_slice()).unwrap_or(data);
            let encoding = Encoding::Identity;
            if let Some(path) =
                glyph_outline_path_cached(font_bytes, glyph, 0, &encoding, to_unicode.as_ref(), 0)
            {
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
                return;
            }
            // Try CIDToGIDMap-aware resolution using cached data.
            if !font_bytes.is_empty()
                && let Ok(face) = ttf_parser::Face::parse(font_bytes, 0)
            {
                let gid = match cid_to_gid {
                    super::font::CIDToGIDMap::Identity => {
                        ttf_parser::GlyphId(glyph.char_code as u16)
                    }
                    super::font::CIDToGIDMap::Explicit(map) => {
                        let cid = glyph.char_code as usize;
                        ttf_parser::GlyphId(map.get(cid).copied().unwrap_or(0))
                    }
                };
                if gid.0 != 0 {
                    let units_per_em = face.units_per_em() as f64;
                    if units_per_em > 0.0 {
                        let scale = glyph.font_size_user_space / units_per_em;
                        let mut builder = GlyphPathBuilder {
                            pb: PathBuilder::new(),
                            x_off: glyph.x as f32,
                            y_off: glyph.y as f32,
                            scale: scale as f32,
                        };
                        if face.outline_glyph(gid, &mut builder).is_some()
                            && let Some(path) = builder.pb.finish()
                        {
                            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
                            return;
                        }
                    }
                }
            }
        }
        None => {}
    }

    // Don't draw fallback rectangles for space/whitespace characters.
    if glyph.char_code == 0x20 || glyph.width_user_space < 0.5 || is_space_glyph(glyph, fonts) {
        return;
    }

    // Fallback: filled rectangle.
    let glyph_w = (glyph.width_user_space * 0.7) as f32;
    let glyph_h = (font_size * 0.75) as f32;
    let x = glyph.x as f32;
    let y = (glyph.y - font_size * 0.2) as f32;

    let mut pb = PathBuilder::new();
    pb.move_to(x, y);
    pb.line_to(x + glyph_w, y);
    pb.line_to(x + glyph_w, y + glyph_h);
    pb.line_to(x, y + glyph_h);
    pb.close();
    if let Some(path) = pb.finish() {
        pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
    }
}

/// Check if a glyph maps to a space character (no visible outline expected).
fn is_space_glyph(glyph: &PositionedGlyph, fonts: &HashMap<Vec<u8>, ResolvedFont>) -> bool {
    let font = match fonts.get(glyph.font_name) {
        Some(f) => f,
        None => return false,
    };

    // Check ToUnicode mapping.
    let to_unicode = match font {
        ResolvedFont::Embedded { to_unicode, .. } => to_unicode.as_ref(),
        ResolvedFont::Standard14 { to_unicode, .. } => to_unicode.as_ref(),
        ResolvedFont::CIDFont { to_unicode, .. } => to_unicode.as_ref(),
    };
    if let Some(tu) = to_unicode
        && let Some(ch) = tu.get(glyph.char_code)
    {
        return ch == ' ' || ch == '\u{00A0}';
    }

    // Check Differences encoding for "space" name.
    let encoding = match font {
        ResolvedFont::Embedded { encoding, .. } => encoding,
        ResolvedFont::Standard14 { encoding, .. } => encoding,
        ResolvedFont::CIDFont { .. } => return false,
    };
    if let Encoding::Differences { diffs, .. } = encoding
        && let Some(name) = diffs.get(&(glyph.char_code as u8))
    {
        return name == "space" || name == "nbspace";
    }

    false
}

/// Build a glyph path using pre-prepared font data (already OTF-wrapped if needed).
/// Skips the per-glyph `wrap_cff_in_otf` call since the cache has already done it.
fn glyph_outline_path_cached(
    prepared_data: &[u8],
    glyph: &PositionedGlyph,
    first_char: u32,
    encoding: &Encoding,
    to_unicode: Option<&ToUnicodeMap>,
    face_index: u32,
) -> Option<Path> {
    let face = ttf_parser::Face::parse(prepared_data, face_index).ok()?;
    build_glyph_path(&face, glyph, first_char, encoding, to_unicode)
}

/// Shared glyph path construction from a parsed font face.
fn build_glyph_path(
    face: &ttf_parser::Face,
    glyph: &PositionedGlyph,
    first_char: u32,
    encoding: &Encoding,
    to_unicode: Option<&ToUnicodeMap>,
) -> Option<Path> {
    let units_per_em = face.units_per_em() as f64;
    if units_per_em == 0.0 {
        return None;
    }

    let code = glyph.char_code;
    let glyph_id = resolve_glyph_id(face, code, first_char, encoding, to_unicode)?;
    if glyph_id.0 == 0 {
        return None;
    }

    let scale = glyph.font_size_user_space / units_per_em;
    let mut builder = GlyphPathBuilder {
        pb: PathBuilder::new(),
        x_off: glyph.x as f32,
        y_off: glyph.y as f32,
        scale: scale as f32,
    };

    face.outline_glyph(glyph_id, &mut builder)?;
    builder.pb.finish()
}

/// Resolve a character code to a GlyphId using the encoding-aware pipeline.
fn resolve_glyph_id(
    face: &ttf_parser::Face,
    code: u32,
    first_char: u32,
    encoding: &Encoding,
    to_unicode: Option<&ToUnicodeMap>,
) -> Option<ttf_parser::GlyphId> {
    use super::encoding::{
        agl_name_to_unicode, macroman_name, resolve_glyph_by_name, standard_name, winansi_name,
    };

    // Strategy 0: ToUnicode CMap (highest priority).
    if let Some(tu) = to_unicode
        && let Some(unicode_char) = tu.get(code)
    {
        let gid = face.glyph_index(unicode_char);
        if gid.is_some() && gid != Some(ttf_parser::GlyphId(0)) {
            return gid;
        }
    }

    // Strategy 1: Encoding Differences.
    if let Encoding::Differences { diffs, base, .. } = encoding {
        if let Some(glyph_name) = diffs.get(&(code as u8)) {
            if let Some(gid) = resolve_glyph_by_name(face, glyph_name) {
                return Some(gid);
            }
            if let Some(unicode_char) = agl_name_to_unicode(glyph_name)
                && let Some(gid) = face.glyph_index(unicode_char)
                && gid.0 != 0
            {
                return Some(gid);
            }
        }
        let name = match base.as_str() {
            "MacRomanEncoding" => macroman_name(code as u8),
            "StandardEncoding" => standard_name(code as u8),
            _ => winansi_name(code as u8),
        };
        if let Some(glyph_name) = name
            && let Some(gid) = resolve_glyph_by_name(face, glyph_name)
        {
            return Some(gid);
        }
    }

    // Strategy 2: Named encoding table.
    if let Encoding::Named(enc_name) = encoding {
        let name = match enc_name.as_str() {
            "MacRomanEncoding" => macroman_name(code as u8),
            "StandardEncoding" => standard_name(code as u8),
            _ => winansi_name(code as u8),
        };
        if let Some(glyph_name) = name
            && let Some(gid) = resolve_glyph_by_name(face, glyph_name)
        {
            return Some(gid);
        }
    }

    // Strategy 3: cmap lookup using char_code as Unicode.
    if let Some(ch) = char::from_u32(code) {
        let gid = face.glyph_index(ch);
        if gid.is_some() && gid != Some(ttf_parser::GlyphId(0)) {
            return gid;
        }
    }

    // Strategy 4: Direct glyph index.
    let direct_id = ttf_parser::GlyphId(code as u16);
    if face
        .outline_glyph(direct_id, &mut NullOutlineBuilder)
        .is_some()
    {
        return Some(direct_id);
    }

    // Strategy 5: Offset by first_char.
    if code >= first_char {
        let offset_id = ttf_parser::GlyphId((code - first_char) as u16);
        if face
            .outline_glyph(offset_id, &mut NullOutlineBuilder)
            .is_some()
        {
            return Some(offset_id);
        }
    }

    None
}

/// Wrap raw CFF (Type1C) data in a minimal OpenType container so ttf-parser can parse it.
/// Includes the minimum required tables: head, hhea, maxp, and CFF.
pub(super) fn wrap_cff_in_otf(cff_data: &[u8]) -> Vec<u8> {
    // Minimal head table (54 bytes)
    #[rustfmt::skip]
    let head: [u8; 54] = [
        0x00, 0x01, 0x00, 0x00, // majorVersion=1, minorVersion=0
        0x00, 0x01, 0x00, 0x00, // fontRevision=1.0
        0x00, 0x00, 0x00, 0x00, // checksumAdjustment (placeholder)
        0x5F, 0x0F, 0x3C, 0xF5, // magicNumber
        0x00, 0x0B,             // flags
        0x03, 0xE8,             // unitsPerEm = 1000
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // created
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // modified
        0x00, 0x00,             // xMin = 0
        0x00, 0x00,             // yMin = 0
        0x03, 0xE8,             // xMax = 1000
        0x03, 0xE8,             // yMax = 1000
        0x00, 0x00,             // macStyle
        0x00, 0x08,             // lowestRecPPEM = 8
        0x00, 0x02,             // fontDirectionHint
        0x00, 0x01,             // indexToLocFormat = 1 (long)
        0x00, 0x00,             // glyphDataFormat
    ];

    // Minimal hhea table (36 bytes)
    #[rustfmt::skip]
    let hhea: [u8; 36] = [
        0x00, 0x01, 0x00, 0x00, // majorVersion=1, minorVersion=0
        0x03, 0x20,             // ascender = 800
        0xFF, 0x38,             // descender = -200
        0x00, 0x00,             // lineGap = 0
        0x03, 0xE8,             // advanceWidthMax = 1000
        0x00, 0x00,             // minLeftSideBearing
        0x00, 0x00,             // minRightSideBearing
        0x03, 0xE8,             // xMaxExtent = 1000
        0x00, 0x01,             // caretSlopeRise = 1
        0x00, 0x00,             // caretSlopeRun = 0
        0x00, 0x00,             // caretOffset
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // reserved
        0x00, 0x00,             // metricDataFormat
        0x00, 0x01,             // numberOfHMetrics = 1
    ];

    // Minimal maxp table (6 bytes for CFF)
    #[rustfmt::skip]
    let maxp: [u8; 6] = [
        0x00, 0x00, 0x50, 0x00, // version = 0.5 (CFF)
        0x00, 0xFF,             // numGlyphs = 255 (generous default)
    ];

    let num_tables: u16 = 4;
    let header_size = 12u32;
    let record_size = 16u32;
    let records_end = header_size + num_tables as u32 * record_size; // 12 + 64 = 76

    // Pad each table to 4-byte boundary
    fn pad4(n: usize) -> usize {
        (n + 3) & !3
    }

    let head_offset = records_end;
    let hhea_offset = head_offset + pad4(head.len()) as u32;
    let maxp_offset = hhea_offset + pad4(hhea.len()) as u32;
    let cff_offset = maxp_offset + pad4(maxp.len()) as u32;
    let total = cff_offset as usize + cff_data.len();

    fn table_checksum(data: &[u8]) -> u32 {
        let mut sum = 0u32;
        let mut i = 0;
        while i + 4 <= data.len() {
            sum = sum.wrapping_add(u32::from_be_bytes([
                data[i],
                data[i + 1],
                data[i + 2],
                data[i + 3],
            ]));
            i += 4;
        }
        if i < data.len() {
            let mut tail = [0u8; 4];
            for (j, &b) in data[i..].iter().enumerate() {
                tail[j] = b;
            }
            sum = sum.wrapping_add(u32::from_be_bytes(tail));
        }
        sum
    }

    let mut otf = Vec::with_capacity(total);

    // Offset table header
    otf.extend_from_slice(b"OTTO");
    otf.extend_from_slice(&num_tables.to_be_bytes());
    otf.extend_from_slice(&32u16.to_be_bytes()); // searchRange
    otf.extend_from_slice(&1u16.to_be_bytes()); // entrySelector
    otf.extend_from_slice(&32u16.to_be_bytes()); // rangeShift

    // Table records (must be sorted by tag)
    // CFF  (0x43464620)
    otf.extend_from_slice(b"CFF ");
    otf.extend_from_slice(&table_checksum(cff_data).to_be_bytes());
    otf.extend_from_slice(&cff_offset.to_be_bytes());
    otf.extend_from_slice(&(cff_data.len() as u32).to_be_bytes());

    // head (0x68656164)
    otf.extend_from_slice(b"head");
    otf.extend_from_slice(&table_checksum(&head).to_be_bytes());
    otf.extend_from_slice(&head_offset.to_be_bytes());
    otf.extend_from_slice(&(head.len() as u32).to_be_bytes());

    // hhea (0x68686561)
    otf.extend_from_slice(b"hhea");
    otf.extend_from_slice(&table_checksum(&hhea).to_be_bytes());
    otf.extend_from_slice(&hhea_offset.to_be_bytes());
    otf.extend_from_slice(&(hhea.len() as u32).to_be_bytes());

    // maxp (0x6D617870)
    otf.extend_from_slice(b"maxp");
    otf.extend_from_slice(&table_checksum(&maxp).to_be_bytes());
    otf.extend_from_slice(&maxp_offset.to_be_bytes());
    otf.extend_from_slice(&(maxp.len() as u32).to_be_bytes());

    // Table data
    otf.extend_from_slice(&head);
    while otf.len() % 4 != 0 {
        otf.push(0);
    }
    otf.extend_from_slice(&hhea);
    while otf.len() % 4 != 0 {
        otf.push(0);
    }
    otf.extend_from_slice(&maxp);
    while otf.len() % 4 != 0 {
        otf.push(0);
    }
    otf.extend_from_slice(cff_data);

    otf
}

/// No-op outline builder for testing if a glyph has outlines.
struct NullOutlineBuilder;
impl ttf_parser::OutlineBuilder for NullOutlineBuilder {
    fn move_to(&mut self, _x: f32, _y: f32) {}
    fn line_to(&mut self, _x: f32, _y: f32) {}
    fn quad_to(&mut self, _x1: f32, _y1: f32, _x: f32, _y: f32) {}
    fn curve_to(&mut self, _x1: f32, _y1: f32, _x2: f32, _y2: f32, _x: f32, _y: f32) {}
    fn close(&mut self) {}
}

/// Adapter from ttf_parser::OutlineBuilder to tiny_skia::PathBuilder.
/// Converts font-unit coordinates to user-space coordinates, flipping Y
/// (font Y-up -> PDF Y-up, but we apply the base transform later which flips).
struct GlyphPathBuilder {
    pb: PathBuilder,
    x_off: f32,
    y_off: f32,
    scale: f32,
}

impl GlyphPathBuilder {
    fn tx(&self, x: f32) -> f32 {
        self.x_off + x * self.scale
    }
    fn ty(&self, y: f32) -> f32 {
        // Font coordinates are Y-up, PDF user space is also Y-up.
        // The base transform handles the flip to pixel space.
        self.y_off + y * self.scale
    }
}

impl ttf_parser::OutlineBuilder for GlyphPathBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pb.move_to(self.tx(x), self.ty(y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.pb.line_to(self.tx(x), self.ty(y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.pb
            .quad_to(self.tx(x1), self.ty(y1), self.tx(x), self.ty(y));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.pb.cubic_to(
            self.tx(x1),
            self.ty(y1),
            self.tx(x2),
            self.ty(y2),
            self.tx(x),
            self.ty(y),
        );
    }
    fn close(&mut self) {
        self.pb.close();
    }
}
