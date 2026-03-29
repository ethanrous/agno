//! PDF text state operators, glyph positioning, and text-to-path conversion.
//!
//! Processes text operators between BT and ET to produce positioned glyphs
//! using the PDF text state machine (ISO 32000-1 section 9.4).

use std::collections::HashMap;
use std::error::Error;

use super::content::Operator;
use super::font::{char_width, ResolvedFont};
use super::graphics::{GraphicsState, Matrix};

/// Positioned glyph for rendering.
#[derive(Debug)]
pub struct PositionedGlyph {
    pub x: f64,
    pub y: f64,
    pub char_code: u8,
    /// Advance width in user space (already scaled by text matrix).
    pub width_user_space: f64,
    /// Effective font size in user space (Tf size * text matrix scale).
    pub font_size_user_space: f64,
    /// Font name for looking up the ResolvedFont during rendering.
    pub font_name: Vec<u8>,
}

/// Process text operators (between BT and ET) and produce positioned glyphs.
pub fn layout_text(
    ops: &[Operator],
    state: &GraphicsState,
    fonts: &HashMap<Vec<u8>, ResolvedFont>,
) -> Result<Vec<PositionedGlyph>, Box<dyn Error>> {
    let mut glyphs = Vec::new();
    let mut tm = Matrix::identity();
    let mut tlm = Matrix::identity();
    let mut font_name = state.font_name.clone();
    let mut font_size = state.font_size;
    let mut char_spacing = state.char_spacing;
    let mut word_spacing = state.word_spacing;
    let mut text_leading = state.text_leading;

    for op in ops {
        match op.name.as_slice() {
            b"Tf" => {
                if op.operands.len() >= 2 {
                    font_name = op.operands[0].as_name().unwrap_or_default().to_vec();
                    font_size = op.operands[1].as_f64().unwrap_or(12.0);
                }
            }
            b"Tc" => {
                char_spacing = op.operands.first().and_then(|o| o.as_f64()).unwrap_or(0.0);
            }
            b"Tw" => {
                word_spacing = op.operands.first().and_then(|o| o.as_f64()).unwrap_or(0.0);
            }
            b"TL" => {
                text_leading = op.operands.first().and_then(|o| o.as_f64()).unwrap_or(0.0);
            }
            b"Td" => {
                if op.operands.len() >= 2 {
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
            }
            b"TD" => {
                if op.operands.len() >= 2 {
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
            }
            b"Tm" => {
                if op.operands.len() >= 6 {
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
            }
            b"T*" => {
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
            b"Tj" => {
                if let Some(text) = op.operands.first().and_then(|o| o.as_string_bytes()) {
                    show_string(
                        text,
                        &mut tm,
                        &font_name,
                        font_size,
                        char_spacing,
                        word_spacing,
                        fonts,
                        &mut glyphs,
                    );
                }
            }
            b"TJ" => {
                if let Some(arr) = op.operands.first().and_then(|o| o.as_array()) {
                    for item in arr {
                        if let Some(text) = item.as_string_bytes() {
                            show_string(
                                text,
                                &mut tm,
                                &font_name,
                                font_size,
                                char_spacing,
                                word_spacing,
                                fonts,
                                &mut glyphs,
                            );
                        } else if let Some(adj) = item.as_f64() {
                            let dx = -(adj / 1000.0) * font_size;
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
            b"'" => {
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
                        &font_name,
                        font_size,
                        char_spacing,
                        word_spacing,
                        fonts,
                        &mut glyphs,
                    );
                }
            }
            b"\"" => {
                if op.operands.len() >= 3 {
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
                            &font_name,
                            font_size,
                            char_spacing,
                            word_spacing,
                            fonts,
                            &mut glyphs,
                        );
                    }
                }
            }
            _ => {}
        }
    }
    Ok(glyphs)
}

fn show_string(
    text: &[u8],
    tm: &mut Matrix,
    font_name: &[u8],
    font_size: f64,
    char_spacing: f64,
    word_spacing: f64,
    fonts: &HashMap<Vec<u8>, ResolvedFont>,
    glyphs: &mut Vec<PositionedGlyph>,
) {
    let font = fonts.get(font_name);
    // Compute the effective font size in user space by measuring how the text
    // matrix scales a unit vector. The y-scale of Tm * font_size gives the
    // visual font size.
    let tm_y_scale = (tm.b * tm.b + tm.d * tm.d).sqrt();
    let effective_font_size = font_size * tm_y_scale;

    for &byte in text {
        let (tx, ty) = tm.transform_point(0.0, 0.0);
        let w = font.map(|f| char_width(f, byte)).unwrap_or(500.0);
        let mut advance_text = (w / 1000.0) * font_size + char_spacing;
        if byte == b' ' {
            advance_text += word_spacing;
        }
        // Compute user-space width: advance through text matrix
        let (ax, ay) = tm.transform_point(advance_text, 0.0);
        let width_user = ((ax - tx) * (ax - tx) + (ay - ty) * (ay - ty)).sqrt();

        glyphs.push(PositionedGlyph {
            x: tx,
            y: ty,
            char_code: byte,
            width_user_space: width_user,
            font_size_user_space: effective_font_size,
            font_name: font_name.to_vec(),
        });
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
    use super::*;
    use super::super::content::parse_content_stream;
    use super::super::font::{standard14_widths, Encoding, ResolvedFont};
    use super::super::graphics::GraphicsStateStack;

    fn make_fonts() -> HashMap<Vec<u8>, ResolvedFont> {
        let mut fonts = HashMap::new();
        fonts.insert(
            b"F1".to_vec(),
            ResolvedFont::Standard14 {
                name: "Helvetica".into(),
                widths: standard14_widths("Helvetica"),
                encoding: Encoding::Named("WinAnsiEncoding".into()),
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

        let state = default_state();
        let fonts = make_fonts();
        let glyphs = layout_text(&text_ops, &state, &fonts).unwrap();

        assert_eq!(glyphs.len(), 5);
        assert!((glyphs[0].x - 100.0).abs() < 0.01);
        assert!((glyphs[0].y - 700.0).abs() < 0.01);
        assert!(glyphs[1].x > glyphs[0].x);
    }

    #[test]
    fn tj_array_with_kerning() {
        let stream = b"BT /F1 12 Tf 0 0 Td [(H) -100 (ello)] TJ ET";
        let text_ops = extract_text_ops(stream);

        let state = default_state();
        let fonts = make_fonts();
        let glyphs = layout_text(&text_ops, &state, &fonts).unwrap();
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

        let state = default_state();
        let fonts = make_fonts();
        let glyphs = layout_text(&text_ops, &state, &fonts).unwrap();

        let line1_y = glyphs[0].y;
        let line2_start = glyphs
            .iter()
            .position(|g| g.char_code == b'L' && (g.y - line1_y).abs() > 0.01);
        assert!(line2_start.is_some());
        let line2_y = glyphs[line2_start.unwrap()].y;
        assert!(
            (line2_y - 86.0).abs() < 0.01,
            "Expected 86, got {line2_y}"
        );
    }

    #[test]
    fn empty_text_block() {
        let stream = b"BT ET";
        let text_ops = extract_text_ops(stream);

        let state = default_state();
        let fonts = HashMap::new();
        let glyphs = layout_text(&text_ops, &state, &fonts).unwrap();
        assert!(glyphs.is_empty());
    }
}
