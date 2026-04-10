//! PDF text state operators, glyph positioning, and text-to-path conversion.
//!
//! Processes text operators between BT and ET to produce positioned glyphs
//! using the PDF text state machine (ISO 32000-1 section 9.4).

use std::collections::HashMap;
use std::error::Error;

use super::content::Operator;
use super::font::{char_width_u32, ResolvedFont};
use super::graphics::{GraphicsState, Matrix};

/// Positioned glyph for rendering.
#[derive(Debug)]
pub struct PositionedGlyph<'a> {
    pub x: f64,
    pub y: f64,
    pub char_code: u32,
    /// Advance width in user space (already scaled by text matrix).
    /// Populated during layout for test assertions; no production consumer yet.
    #[allow(dead_code)]
    pub width_user_space: f64,
    /// Effective font size in user space (Tf size * text matrix scale).
    pub font_size_user_space: f64,
    /// Font name for looking up the ResolvedFont during rendering.
    pub font_name: &'a [u8],
}

/// Process text operators (between BT and ET) and produce positioned glyphs.
pub fn layout_text<'a>(
    ops: &[&'a Operator],
    state: &'a GraphicsState,
    fonts: &HashMap<Vec<u8>, ResolvedFont>,
) -> Result<Vec<PositionedGlyph<'a>>, Box<dyn Error>> {
    let mut glyphs = Vec::new();
    let mut tm = Matrix::identity();
    let mut tlm = Matrix::identity();
    let mut font_name: &'a [u8] = &state.font_name;
    let mut font_size = state.font_size;
    let mut char_spacing = state.char_spacing;
    let mut word_spacing = state.word_spacing;
    let mut text_leading = state.text_leading;
    let mut horizontal_scaling = state.horizontal_scaling;
    let mut text_rise = state.text_rise;
    let mut text_rendering_mode = state.text_rendering_mode;

    for op in ops {
        match (op.name.as_slice(), op.operands.len()) {
            (b"Tf", 2..) => {
                font_name = op.operands[0].as_name().unwrap_or_default();
                font_size = op.operands[1].as_f64().unwrap_or(12.0);
            }
            (b"Tc", _) => {
                char_spacing = op.operands.first().and_then(|o| o.as_f64()).unwrap_or(0.0);
            }
            (b"Tw", _) => {
                word_spacing = op.operands.first().and_then(|o| o.as_f64()).unwrap_or(0.0);
            }
            (b"TL", _) => {
                text_leading = op.operands.first().and_then(|o| o.as_f64()).unwrap_or(0.0);
            }
            (b"Tz", _) => {
                horizontal_scaling = op
                    .operands
                    .first()
                    .and_then(|o| o.as_f64())
                    .unwrap_or(100.0);
            }
            (b"Ts", _) => {
                text_rise = op.operands.first().and_then(|o| o.as_f64()).unwrap_or(0.0);
            }
            (b"Tr", _) => {
                text_rendering_mode =
                    op.operands.first().and_then(|o| o.as_f64()).unwrap_or(0.0) as u8;
            }
            (b"Td", 2..) => {
                let tx = op.operands[0].as_f64().unwrap_or(0.0);
                let ty = op.operands[1].as_f64().unwrap_or(0.0);
                let translate = Matrix {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    e: tx,
                    f: ty,
                };
                tlm = translate.concat(&tlm);
                tm = tlm;
            }
            (b"TD", 2..) => {
                let tx = op.operands[0].as_f64().unwrap_or(0.0);
                let ty = op.operands[1].as_f64().unwrap_or(0.0);
                text_leading = -ty;
                let translate = Matrix {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    e: tx,
                    f: ty,
                };
                tlm = translate.concat(&tlm);
                tm = tlm;
            }
            (b"Tm", 6..) => {
                tm = Matrix {
                    a: op.operands[0].as_f64().unwrap_or(1.0),
                    b: op.operands[1].as_f64().unwrap_or(0.0),
                    c: op.operands[2].as_f64().unwrap_or(0.0),
                    d: op.operands[3].as_f64().unwrap_or(1.0),
                    e: op.operands[4].as_f64().unwrap_or(0.0),
                    f: op.operands[5].as_f64().unwrap_or(0.0),
                };
                tlm = tm;
            }
            (b"T*", _) => {
                let translate = Matrix {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    e: 0.0,
                    f: -text_leading,
                };
                tlm = translate.concat(&tlm);
                tm = tlm;
            }
            (b"Tj", _) => {
                if let Some(text) = op.operands.first().and_then(|o| o.as_string_bytes()) {
                    show_string(
                        text,
                        &mut tm,
                        font_name,
                        font_size,
                        char_spacing,
                        word_spacing,
                        horizontal_scaling,
                        text_rise,
                        text_rendering_mode,
                        fonts,
                        &mut glyphs,
                    );
                }
            }
            (b"TJ", _) => {
                if let Some(arr) = op.operands.first().and_then(|o| o.as_array()) {
                    for item in arr {
                        if let Some(text) = item.as_string_bytes() {
                            show_string(
                                text,
                                &mut tm,
                                font_name,
                                font_size,
                                char_spacing,
                                word_spacing,
                                horizontal_scaling,
                                text_rise,
                                text_rendering_mode,
                                fonts,
                                &mut glyphs,
                            );
                        } else if let Some(adj) = item.as_f64() {
                            let dx = -(adj / 1000.0) * font_size * (horizontal_scaling / 100.0);
                            let advance = Matrix {
                                a: 1.0,
                                b: 0.0,
                                c: 0.0,
                                d: 1.0,
                                e: dx,
                                f: 0.0,
                            };
                            tm = advance.concat(&tm);
                        }
                    }
                }
            }
            (b"'", _) => {
                let translate = Matrix {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    e: 0.0,
                    f: -text_leading,
                };
                tlm = translate.concat(&tlm);
                tm = tlm;
                if let Some(text) = op.operands.first().and_then(|o| o.as_string_bytes()) {
                    show_string(
                        text,
                        &mut tm,
                        font_name,
                        font_size,
                        char_spacing,
                        word_spacing,
                        horizontal_scaling,
                        text_rise,
                        text_rendering_mode,
                        fonts,
                        &mut glyphs,
                    );
                }
            }
            (b"\"", 3..) => {
                word_spacing = op.operands[0].as_f64().unwrap_or(0.0);
                char_spacing = op.operands[1].as_f64().unwrap_or(0.0);
                let translate = Matrix {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    e: 0.0,
                    f: -text_leading,
                };
                tlm = translate.concat(&tlm);
                tm = tlm;
                if let Some(text) = op.operands[2].as_string_bytes() {
                    show_string(
                        text,
                        &mut tm,
                        font_name,
                        font_size,
                        char_spacing,
                        word_spacing,
                        horizontal_scaling,
                        text_rise,
                        text_rendering_mode,
                        fonts,
                        &mut glyphs,
                    );
                }
            }
            _ => {}
        }
    }
    Ok(glyphs)
}

#[allow(clippy::too_many_arguments)]
fn show_string<'a>(
    text: &[u8],
    tm: &mut Matrix,
    font_name: &'a [u8],
    font_size: f64,
    char_spacing: f64,
    word_spacing: f64,
    horizontal_scaling: f64,
    text_rise: f64,
    text_rendering_mode: u8,
    fonts: &HashMap<Vec<u8>, ResolvedFont>,
    glyphs: &mut Vec<PositionedGlyph<'a>>,
) {
    let font = fonts.get(font_name);
    let tm_y_scale = (tm.b * tm.b + tm.d * tm.d).sqrt();
    let effective_font_size = font_size * tm_y_scale;
    let th = horizontal_scaling / 100.0;

    let is_two_byte = matches!(
        font,
        Some(ResolvedFont::CIDFont {
            is_two_byte: true,
            ..
        })
    );

    let mut i = 0;
    while i < text.len() {
        let code: u32 = if is_two_byte && i + 1 < text.len() {
            let c = ((text[i] as u32) << 8) | (text[i + 1] as u32);
            i += 2;
            c
        } else {
            let c = text[i] as u32;
            i += 1;
            c
        };

        let (tx, ty) = tm.transform_point(0.0, text_rise);
        let w = font.map(|f| char_width_u32(f, code)).unwrap_or(500.0);
        let mut advance_text = ((w / 1000.0) * font_size + char_spacing) * th;
        if code == 0x20 {
            advance_text += word_spacing * th;
        }
        let (ax, ay) = tm.transform_point(advance_text, text_rise);
        let width_user = ((ax - tx) * (ax - tx) + (ay - ty) * (ay - ty)).sqrt();

        // Rendering mode 3 = invisible: advance position but skip glyph output.
        if text_rendering_mode != 3 {
            glyphs.push(PositionedGlyph {
                x: tx,
                y: ty,
                char_code: code,
                width_user_space: width_user,
                font_size_user_space: effective_font_size,
                font_name,
            });
        }
        let advance_m = Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: advance_text,
            f: 0.0,
        };
        *tm = advance_m.concat(tm);
    }
}

#[cfg(test)]
mod tests {
    use super::super::content::parse_content_stream;
    use super::super::font::{standard14_widths, Encoding, ResolvedFont};
    use super::super::graphics::GraphicsStateStack;
    use super::*;

    fn make_fonts() -> HashMap<Vec<u8>, ResolvedFont> {
        let mut fonts = HashMap::new();
        fonts.insert(
            b"F1".to_vec(),
            ResolvedFont::Standard14 {
                name: "Helvetica".into(),
                widths: standard14_widths("Helvetica"),
                encoding: Encoding::Named("WinAnsiEncoding".into()),
                to_unicode: None,
            },
        );
        fonts
    }

    fn default_state() -> GraphicsState {
        GraphicsStateStack::new().current().clone()
    }

    fn extract_text_ops(stream: &[u8]) -> Vec<Operator> {
        let all_ops = parse_content_stream(stream).unwrap();
        all_ops
            .into_iter()
            .skip_while(|o| o.name != b"BT")
            .skip(1)
            .take_while(|o| o.name != b"ET")
            .collect()
    }

    #[test]
    fn simple_text_positioning() {
        let stream = b"BT /F1 12 Tf 100 700 Td (Hello) Tj ET";
        let text_ops = extract_text_ops(stream);
        let text_op_refs: Vec<&Operator> = text_ops.iter().collect();

        let state = default_state();
        let fonts = make_fonts();
        let glyphs = layout_text(&text_op_refs, &state, &fonts).unwrap();

        assert_eq!(glyphs.len(), 5);
        assert!((glyphs[0].x - 100.0).abs() < 0.01);
        assert!((glyphs[0].y - 700.0).abs() < 0.01);
        assert!(glyphs[1].x > glyphs[0].x);
    }

    #[test]
    fn tj_array_with_kerning() {
        let stream = b"BT /F1 12 Tf 0 0 Td [(H) -100 (ello)] TJ ET";
        let text_ops = extract_text_ops(stream);
        let text_op_refs: Vec<&Operator> = text_ops.iter().collect();

        let state = default_state();
        let fonts = make_fonts();
        let glyphs = layout_text(&text_op_refs, &state, &fonts).unwrap();
        assert_eq!(glyphs.len(), 5);
        let h_advance = glyphs[0].width_user_space;
        let kern = 100.0 / 1000.0 * 12.0;
        assert!(
            (glyphs[1].x - (h_advance + kern)).abs() < 0.1,
            "Expected {}, got {}",
            h_advance + kern,
            glyphs[1].x
        );
    }

    #[test]
    fn text_leading_newline() {
        let stream = b"BT /F1 12 Tf 14 TL 0 100 Td (Line1) Tj T* (Line2) Tj ET";
        let text_ops = extract_text_ops(stream);
        let text_op_refs: Vec<&Operator> = text_ops.iter().collect();

        let state = default_state();
        let fonts = make_fonts();
        let glyphs = layout_text(&text_op_refs, &state, &fonts).unwrap();

        let line1_y = glyphs[0].y;
        let line2_start = glyphs
            .iter()
            .position(|g| g.char_code == b'L' as u32 && (g.y - line1_y).abs() > 0.01);
        assert!(line2_start.is_some());
        let line2_y = glyphs[line2_start.unwrap()].y;
        assert!((line2_y - 86.0).abs() < 0.01, "Expected 86, got {line2_y}");
    }

    #[test]
    fn tj_array_respects_horizontal_scaling() {
        // TJ numeric adjustment should be scaled by Th (horizontal_scaling / 100).
        // PDF spec: tx = -(adj / 1000) * Tfs * Th
        let stream = b"BT /F1 12 Tf 200 Tz 0 0 Td [(A) -500 (B)] TJ ET";
        let text_ops = extract_text_ops(stream);
        let text_op_refs: Vec<&Operator> = text_ops.iter().collect();

        let state = default_state();
        let fonts = make_fonts();
        let glyphs = layout_text(&text_op_refs, &state, &fonts).unwrap();

        assert_eq!(glyphs.len(), 2, "Expected 2 glyphs (A and B)");

        // At Tz=200, th=2.0. The TJ adjustment of -500 means:
        // dx = -(-500 / 1000) * 12 * 2.0 = 12.0 in text coords
        // Glyph A advance at Tz=200: ((667/1000) * 12) * 2.0 = 16.008
        // Total offset for B: 16.008 + 12.0 = 28.008
        // Without Th on TJ: 16.008 + 6.0 = 22.008 — WRONG
        let b_x = glyphs[1].x;
        let expected_with_th = 28.008;
        let wrong_without_th = 22.008;

        assert!(
            (b_x - expected_with_th).abs() < 0.1,
            "B position {b_x} should be near {expected_with_th} (with Th), not {wrong_without_th} (without Th)"
        );
    }

    #[test]
    fn empty_text_block() {
        let stream = b"BT ET";
        let text_ops = extract_text_ops(stream);
        let text_op_refs: Vec<&Operator> = text_ops.iter().collect();

        let state = default_state();
        let fonts = HashMap::new();
        let glyphs = layout_text(&text_op_refs, &state, &fonts).unwrap();
        assert!(glyphs.is_empty());
    }
}
