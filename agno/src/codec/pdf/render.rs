//! PDF page rasterizer using tiny-skia.
//!
//! Converts parsed PDF content streams into RGBA pixels via the tiny-skia 2D
//! rendering library, then composites over a white background to produce RGB8.

use std::collections::HashMap;
use std::error::Error;
use std::sync::OnceLock;

use tiny_skia::{
    FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, Stroke,
    StrokeDash, Transform,
};

use super::content::{parse_content_stream, Operator};
use super::document::PdfDocument;
use super::font::ResolvedFont;
use super::graphics::{Color, GraphicsStateStack, Matrix};
use super::objects::PdfObject;
use super::text::{layout_text, PositionedGlyph};

/// Holds document-level context needed during content stream execution.
struct RenderContext<'a> {
    doc: &'a PdfDocument<'a>,
    resources: PdfObject,
    base: Transform,
    depth: u32,
}

const MAX_XOBJECT_DEPTH: u32 = 10;

/// Render a single PDF page to an RGBA8 pixmap.
///
/// Uses 2x internal supersampling for high-quality anti-aliased output:
/// renders at double the requested resolution, then downscales with
/// bilinear filtering for smooth edges.
pub fn render_page(
    doc: &PdfDocument,
    page_index: usize,
    scale: f32,
) -> Result<Pixmap, Box<dyn Error>> {
    let (x0, y0, x1, y1) = doc.page_media_box(page_index)?;
    let page_w = (x1 - x0).abs();
    let page_h = (y1 - y0).abs();

    let target_w = (page_w * scale as f64).ceil() as u32;
    let target_h = (page_h * scale as f64).ceil() as u32;

    if target_w == 0 || target_h == 0 {
        return Err("Page has zero pixel dimensions".into());
    }

    // Render at 2x for supersampled anti-aliasing.
    let ss = 2.0f32;
    let render_scale = scale * ss;
    let render_w = (page_w * render_scale as f64).ceil() as u32;
    let render_h = (page_h * render_scale as f64).ceil() as u32;

    let mut pixmap = Pixmap::new(render_w, render_h)
        .ok_or("Failed to create pixmap (dimensions too large)")?;
    pixmap.fill(tiny_skia::Color::WHITE);

    let content = doc.page_content_stream(page_index)?;
    let operators = parse_content_stream(&content)?;

    let resources = doc.page_resources(page_index).unwrap_or_else(|_| {
        PdfObject::Dictionary(HashMap::new())
    });
    let fonts = resolve_fonts_from_resources(&resources, doc);

    let base = Transform::from_row(
        render_scale,
        0.0,
        0.0,
        -render_scale,
        -(x0 as f32) * render_scale,
        (y1 as f32) * render_scale,
    );

    let ctx = RenderContext {
        doc,
        resources,
        base,
        depth: 0,
    };

    let mut state = GraphicsStateStack::new();
    let clip_mask: Option<Mask> = None;

    execute_content_stream(&operators, &mut state, &mut pixmap, &ctx, &fonts, clip_mask.as_ref());

    // Downscale from render resolution to target resolution.
    if render_w != target_w || render_h != target_h {
        Ok(downscale_pixmap(&pixmap, target_w, target_h))
    } else {
        Ok(pixmap)
    }
}

/// Downscale a pixmap using box-filter averaging (equivalent to area-based downsampling).
/// This produces high-quality anti-aliased output from a supersampled source.
fn downscale_pixmap(src: &Pixmap, dst_w: u32, dst_h: u32) -> Pixmap {
    let mut dst = Pixmap::new(dst_w, dst_h).unwrap();
    let src_data = src.data();
    let dst_data = dst.data_mut();
    let sw = src.width() as f64;
    let sh = src.height() as f64;
    let dw = dst_w as f64;
    let dh = dst_h as f64;
    let sx = sw / dw;
    let sy = sh / dh;

    for dy in 0..dst_h {
        for dx in 0..dst_w {
            // Source region for this destination pixel
            let src_x0 = (dx as f64 * sx) as u32;
            let src_y0 = (dy as f64 * sy) as u32;
            let src_x1 = ((dx as f64 + 1.0) * sx).ceil() as u32;
            let src_y1 = ((dy as f64 + 1.0) * sy).ceil() as u32;
            let src_x1 = src_x1.min(src.width());
            let src_y1 = src_y1.min(src.height());

            let mut r_sum: u32 = 0;
            let mut g_sum: u32 = 0;
            let mut b_sum: u32 = 0;
            let mut a_sum: u32 = 0;
            let mut count: u32 = 0;

            for py in src_y0..src_y1 {
                for px in src_x0..src_x1 {
                    let i = (py * src.width() + px) as usize * 4;
                    r_sum += src_data[i] as u32;
                    g_sum += src_data[i + 1] as u32;
                    b_sum += src_data[i + 2] as u32;
                    a_sum += src_data[i + 3] as u32;
                    count += 1;
                }
            }

            if count > 0 {
                let di = (dy * dst_w + dx) as usize * 4;
                dst_data[di] = (r_sum / count) as u8;
                dst_data[di + 1] = (g_sum / count) as u8;
                dst_data[di + 2] = (b_sum / count) as u8;
                dst_data[di + 3] = (a_sum / count) as u8;
            }
        }
    }
    dst
}

/// Execute a parsed content stream, handling BT/ET text blocks and dispatching
/// individual operators. Shared by `render_page` and `render_xobject`.
fn execute_content_stream(
    operators: &[Operator],
    state: &mut GraphicsStateStack,
    pixmap: &mut Pixmap,
    ctx: &RenderContext,
    fonts: &HashMap<Vec<u8>, ResolvedFont>,
    initial_clip: Option<&Mask>,
) {
    let mut path_builder = PathBuilder::new();
    let mut clip_pending: Option<FillRule> = None;
    let mut clip_mask: Option<Mask> = initial_clip.cloned();

    let mut i = 0;
    while i < operators.len() {
        if operators[i].name == b"BT" {
            i += 1;
            let bt_start = i;
            while i < operators.len() && operators[i].name != b"ET" {
                // Process non-text state operators (color, graphics state)
                // that appear inside BT/ET blocks.
                let name = operators[i].name.as_slice();
                match name {
                    b"rg" | b"RG" | b"g" | b"G" | b"k" | b"K"
                    | b"cs" | b"CS" | b"sc" | b"SC" | b"scn" | b"SCN"
                    | b"q" | b"Q" | b"gs" => {
                        execute_operator(
                            &operators[i],
                            state,
                            pixmap,
                            ctx,
                            &mut path_builder,
                            &mut clip_pending,
                            &mut clip_mask,
                        );
                    }
                    _ => {}
                }
                i += 1;
            }
            // Collect just the text operators for layout.
            let text_ops: Vec<&Operator> = operators[bt_start..i]
                .iter()
                .filter(|op| matches!(
                    op.name.as_slice(),
                    b"Tf" | b"Td" | b"TD" | b"Tm" | b"T*"
                    | b"Tj" | b"TJ" | b"Tc" | b"Tw" | b"TL"
                    | b"'" | b"\""
                ))
                .collect();

            if !text_ops.is_empty() {
                // Build owned ops slice for layout_text.
                let text_op_refs: Vec<Operator> = text_ops
                    .into_iter()
                    .map(|o| Operator {
                        name: o.name.clone(),
                        operands: o.operands.clone(),
                    })
                    .collect();

                if let Ok(glyphs) = layout_text(&text_op_refs, state.current(), fonts) {
                    let transform = combined_transform(&state.current().ctm, ctx.base);
                    let color = &state.current().fill_color;
                    let alpha = state.current().fill_alpha;

                    let mask_ref = clip_mask.as_ref();
                    for glyph in &glyphs {
                        render_glyph(
                            glyph,
                            fonts,
                            color,
                            alpha,
                            transform,
                            pixmap,
                            mask_ref,
                        );
                    }
                }
            }
            // Skip past ET
            if i < operators.len() {
                i += 1;
            }
        } else {
            execute_operator(
                &operators[i],
                state,
                pixmap,
                ctx,
                &mut path_builder,
                &mut clip_pending,
                &mut clip_mask,
            );
            i += 1;
        }
    }
}

/// Convert a tiny-skia premultiplied RGBA8 Pixmap to RGB8 bytes composited
/// over a white background.
pub fn pixmap_to_rgb8(pixmap: &Pixmap) -> Vec<u8> {
    let rgba = pixmap.data();
    let pixel_count = pixmap.width() as usize * pixmap.height() as usize;
    let mut rgb = Vec::with_capacity(pixel_count * 3);
    for chunk in rgba.chunks_exact(4) {
        // Data is premultiplied: R_pm = R * A / 255.
        // Composite over white: R_out = R_pm + (255 - A).
        let a = chunk[3];
        let inv_a = 255 - a;
        rgb.push(chunk[0].saturating_add(inv_a));
        rgb.push(chunk[1].saturating_add(inv_a));
        rgb.push(chunk[2].saturating_add(inv_a));
    }
    rgb
}

/// Build the combined tiny-skia transform: base * CTM.
fn combined_transform(ctm: &Matrix, base: Transform) -> Transform {
    let ctm_t = Transform::from_row(
        ctm.a as f32,
        ctm.b as f32,
        ctm.c as f32,
        ctm.d as f32,
        ctm.e as f32,
        ctm.f as f32,
    );
    base.pre_concat(ctm_t)
}

fn execute_operator(
    op: &Operator,
    state: &mut GraphicsStateStack,
    pixmap: &mut Pixmap,
    ctx: &RenderContext,
    path_builder: &mut PathBuilder,
    clip_pending: &mut Option<FillRule>,
    clip_mask: &mut Option<Mask>,
) {
    let name = op.name.as_slice();
    let args = &op.operands;

    match name {
        // --- Graphics state ---
        b"q" => state.save(),
        b"Q" => {
            let _ = state.restore();
        }
        b"cm" if args.len() >= 6 => {
            let m = Matrix {
                a: arg_f64(args, 0),
                b: arg_f64(args, 1),
                c: arg_f64(args, 2),
                d: arg_f64(args, 3),
                e: arg_f64(args, 4),
                f: arg_f64(args, 5),
            };
            // PDF cm pre-multiplies: new_CTM = m * current_CTM
            state.current_mut().ctm = m.concat(&state.current().ctm);
        }
        b"w" if !args.is_empty() => {
            state.current_mut().line_width = arg_f64(args, 0);
        }
        b"J" if !args.is_empty() => {
            state.current_mut().line_cap = arg_f64(args, 0) as u8;
        }
        b"j" if !args.is_empty() => {
            state.current_mut().line_join = arg_f64(args, 0) as u8;
        }
        b"M" if !args.is_empty() => {
            state.current_mut().miter_limit = arg_f64(args, 0);
        }
        b"d" if args.len() >= 2 => {
            if let Some(arr) = args[0].as_array() {
                let dashes: Vec<f64> = arr.iter().filter_map(|o| o.as_f64()).collect();
                state.current_mut().dash_array = dashes;
            }
            state.current_mut().dash_phase = arg_f64(args, 1);
        }

        // --- Color ---
        b"rg" if args.len() >= 3 => {
            state.current_mut().fill_color = Color {
                r: arg_f64(args, 0),
                g: arg_f64(args, 1),
                b: arg_f64(args, 2),
            };
        }
        b"RG" if args.len() >= 3 => {
            state.current_mut().stroke_color = Color {
                r: arg_f64(args, 0),
                g: arg_f64(args, 1),
                b: arg_f64(args, 2),
            };
        }
        b"g" if !args.is_empty() => {
            let v = arg_f64(args, 0);
            state.current_mut().fill_color = Color { r: v, g: v, b: v };
        }
        b"G" if !args.is_empty() => {
            let v = arg_f64(args, 0);
            state.current_mut().stroke_color = Color { r: v, g: v, b: v };
        }
        b"k" if args.len() >= 4 => {
            let (c, m, y, k) = (
                arg_f64(args, 0),
                arg_f64(args, 1),
                arg_f64(args, 2),
                arg_f64(args, 3),
            );
            state.current_mut().fill_color = Color {
                r: (1.0 - c) * (1.0 - k),
                g: (1.0 - m) * (1.0 - k),
                b: (1.0 - y) * (1.0 - k),
            };
        }
        b"K" if args.len() >= 4 => {
            let (c, m, y, k) = (
                arg_f64(args, 0),
                arg_f64(args, 1),
                arg_f64(args, 2),
                arg_f64(args, 3),
            );
            state.current_mut().stroke_color = Color {
                r: (1.0 - c) * (1.0 - k),
                g: (1.0 - m) * (1.0 - k),
                b: (1.0 - y) * (1.0 - k),
            };
        }

        // --- Named color space selection (accept silently) ---
        b"cs" | b"CS" => {}

        // --- sc/SC: set color in current color space (DeviceGray-style) ---
        b"sc" if !args.is_empty() => {
            state.current_mut().fill_color = color_from_operands_gray(args);
        }
        b"SC" if !args.is_empty() => {
            state.current_mut().stroke_color = color_from_operands_gray(args);
        }

        // --- scn/SCN: set color in named color space (Separation tint model) ---
        b"scn" if !args.is_empty() => {
            let numeric: Vec<&PdfObject> = args.iter().filter(|a| a.as_f64().is_some()).collect();
            if !numeric.is_empty() {
                state.current_mut().fill_color = color_from_operand_refs_tint(&numeric);
            }
        }
        b"SCN" if !args.is_empty() => {
            let numeric: Vec<&PdfObject> = args.iter().filter(|a| a.as_f64().is_some()).collect();
            if !numeric.is_empty() {
                state.current_mut().stroke_color = color_from_operand_refs_tint(&numeric);
            }
        }

        // --- Path construction ---
        b"m" if args.len() >= 2 => {
            path_builder.move_to(arg_f32(args, 0), arg_f32(args, 1));
        }
        b"l" if args.len() >= 2 => {
            path_builder.line_to(arg_f32(args, 0), arg_f32(args, 1));
        }
        b"c" if args.len() >= 6 => {
            path_builder.cubic_to(
                arg_f32(args, 0),
                arg_f32(args, 1),
                arg_f32(args, 2),
                arg_f32(args, 3),
                arg_f32(args, 4),
                arg_f32(args, 5),
            );
        }
        b"v" if args.len() >= 4 => {
            // Current point used as first control point.
            // PathBuilder doesn't expose current point, so approximate with cubic_to
            // using the second control point as the first.
            path_builder.cubic_to(
                arg_f32(args, 0),
                arg_f32(args, 1),
                arg_f32(args, 0),
                arg_f32(args, 1),
                arg_f32(args, 2),
                arg_f32(args, 3),
            );
        }
        b"y" if args.len() >= 4 => {
            // End point used as second control point.
            path_builder.cubic_to(
                arg_f32(args, 0),
                arg_f32(args, 1),
                arg_f32(args, 2),
                arg_f32(args, 3),
                arg_f32(args, 2),
                arg_f32(args, 3),
            );
        }
        b"h" => {
            path_builder.close();
        }
        b"re" if args.len() >= 4 => {
            let (x, y, w, h) = (
                arg_f32(args, 0),
                arg_f32(args, 1),
                arg_f32(args, 2),
                arg_f32(args, 3),
            );
            path_builder.move_to(x, y);
            path_builder.line_to(x + w, y);
            path_builder.line_to(x + w, y + h);
            path_builder.line_to(x, y + h);
            path_builder.close();
        }

        // --- Path painting ---
        b"S" => {
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, false, true, FillRule::Winding);
        }
        b"s" => {
            path_builder.close();
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, false, true, FillRule::Winding);
        }
        b"f" | b"F" => {
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, true, false, FillRule::Winding);
        }
        b"f*" => {
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, true, false, FillRule::EvenOdd);
        }
        b"B" => {
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, true, true, FillRule::Winding);
        }
        b"B*" => {
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, true, true, FillRule::EvenOdd);
        }
        b"b" => {
            path_builder.close();
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, true, true, FillRule::Winding);
        }
        b"b*" => {
            path_builder.close();
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, true, true, FillRule::EvenOdd);
        }
        b"n" => {
            // End path without painting (clipping only).
            apply_pending_clip(path_builder, clip_pending, clip_mask, pixmap, state, ctx.base);
            *path_builder = PathBuilder::new();
        }

        // --- Clipping ---
        b"W" => {
            *clip_pending = Some(FillRule::Winding);
        }
        b"W*" => {
            *clip_pending = Some(FillRule::EvenOdd);
        }

        // --- Text (handled in execute_content_stream BT/ET loop) ---
        b"BT" | b"ET" | b"Tf" | b"Td" | b"TD" | b"Tm" | b"T*" | b"Tj" | b"TJ"
        | b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Tr" | b"Ts" | b"'" | b"\"" => {}

        // --- XObject ---
        b"Do" if !args.is_empty() => {
            if ctx.depth < MAX_XOBJECT_DEPTH {
                if let Some(xobj_name) = args[0].as_name() {
                    render_xobject(xobj_name, state, pixmap, ctx, clip_mask.as_ref());
                }
            }
        }

        // --- Extended graphics state ---
        b"gs" => {}

        // --- Inline image (already consumed by parser) ---
        b"BI" => {}

        // --- Marked content ---
        b"BMC" | b"BDC" | b"EMC" | b"MP" | b"DP" => {}

        // --- Compatibility ---
        b"BX" | b"EX" => {}

        // Unknown operators: silently ignore.
        _ => {}
    }
}

/// Render a Form XObject by recursively executing its content stream.
fn render_xobject(
    name: &[u8],
    state: &mut GraphicsStateStack,
    pixmap: &mut Pixmap,
    ctx: &RenderContext,
    clip: Option<&Mask>,
) {
    // 1. Look up /XObject/<name> in resources
    let xobj_dict = ctx.resources.get(b"XObject")
        .and_then(|x| ctx.doc.resolve_value(x).ok());
    let xobj_ref = xobj_dict.as_ref()
        .and_then(|d| d.get(name))
        .cloned();
    let xobj = match xobj_ref.and_then(|r| ctx.doc.resolve_value(&r).ok()) {
        Some(o) => o,
        None => return,
    };

    let subtype = xobj.get_name_str(b"Subtype").unwrap_or("");
    if subtype != "Form" {
        return;
    }

    // 2. Get stream data
    let stream_data = match &xobj {
        PdfObject::Stream { data, .. } => data.clone(),
        _ => return,
    };

    // 3. Parse the Form's content stream
    let content_ops = match parse_content_stream(&stream_data) {
        Ok(ops) => ops,
        Err(_) => return,
    };

    // 4. Save state, apply Form matrix
    state.save();
    if let Some(matrix_arr) = xobj.get(b"Matrix").and_then(|m| m.as_array()) {
        if matrix_arr.len() >= 6 {
            let m = Matrix {
                a: matrix_arr[0].as_f64().unwrap_or(1.0),
                b: matrix_arr[1].as_f64().unwrap_or(0.0),
                c: matrix_arr[2].as_f64().unwrap_or(0.0),
                d: matrix_arr[3].as_f64().unwrap_or(1.0),
                e: matrix_arr[4].as_f64().unwrap_or(0.0),
                f: matrix_arr[5].as_f64().unwrap_or(0.0),
            };
            state.current_mut().ctm = m.concat(&state.current().ctm);
        }
    }

    // 5. Get Form resources (or inherit from parent)
    let form_resources = xobj.get(b"Resources")
        .and_then(|r| ctx.doc.resolve_value(r).ok())
        .unwrap_or_else(|| ctx.resources.clone());

    // 6. Resolve fonts for this form's resources
    let form_fonts = resolve_fonts_from_resources(&form_resources, ctx.doc);

    // 7. Build child context with incremented depth
    let child_ctx = RenderContext {
        doc: ctx.doc,
        resources: form_resources,
        base: ctx.base,
        depth: ctx.depth + 1,
    };

    // 8. Execute the form's content stream
    execute_content_stream(&content_ops, state, pixmap, &child_ctx, &form_fonts, clip);

    // 9. Restore state
    let _ = state.restore();
}

/// Interpret color operands for `sc`/`SC` (DeviceGray-style: 1.0 = white).
fn color_from_operands_gray(args: &[PdfObject]) -> Color {
    match args.len() {
        1 => {
            let v = args[0].as_f64().unwrap_or(0.0);
            Color { r: v, g: v, b: v }
        }
        3 => Color {
            r: args[0].as_f64().unwrap_or(0.0),
            g: args[1].as_f64().unwrap_or(0.0),
            b: args[2].as_f64().unwrap_or(0.0),
        },
        4 => cmyk_to_color(
            args[0].as_f64().unwrap_or(0.0),
            args[1].as_f64().unwrap_or(0.0),
            args[2].as_f64().unwrap_or(0.0),
            args[3].as_f64().unwrap_or(0.0),
        ),
        _ => Color::black(),
    }
}

/// Interpret color operands for `scn`/`SCN` (Separation tint model: 1.0 = full ink = dark).
fn color_from_operand_refs_tint(args: &[&PdfObject]) -> Color {
    match args.len() {
        1 => {
            let v = args[0].as_f64().unwrap_or(0.0);
            // Separation tint: 1.0 = full ink = dark
            Color { r: 1.0 - v, g: 1.0 - v, b: 1.0 - v }
        }
        3 => Color {
            r: args[0].as_f64().unwrap_or(0.0),
            g: args[1].as_f64().unwrap_or(0.0),
            b: args[2].as_f64().unwrap_or(0.0),
        },
        4 => cmyk_to_color(
            args[0].as_f64().unwrap_or(0.0),
            args[1].as_f64().unwrap_or(0.0),
            args[2].as_f64().unwrap_or(0.0),
            args[3].as_f64().unwrap_or(0.0),
        ),
        _ => Color::black(),
    }
}

fn cmyk_to_color(c: f64, m: f64, y: f64, k: f64) -> Color {
    Color {
        r: (1.0 - c) * (1.0 - k),
        g: (1.0 - m) * (1.0 - k),
        b: (1.0 - y) * (1.0 - k),
    }
}

/// Finalize the current path, optionally fill and/or stroke, apply pending clip,
/// then reset the path builder.
fn paint_path(
    state: &mut GraphicsStateStack,
    pixmap: &mut Pixmap,
    base: Transform,
    path_builder: &mut PathBuilder,
    clip_pending: &mut Option<FillRule>,
    clip_mask: &mut Option<Mask>,
    do_fill: bool,
    do_stroke: bool,
    fill_rule: FillRule,
) {
    let built = std::mem::replace(path_builder, PathBuilder::new());
    let path = match built.finish() {
        Some(p) => p,
        None => {
            // Empty or degenerate path — still apply pending clip.
            apply_pending_clip(path_builder, clip_pending, clip_mask, pixmap, state, base);
            return;
        }
    };

    let transform = combined_transform(&state.current().ctm, base);
    let mask_ref = clip_mask.as_ref();

    if do_fill {
        fill_path(&path, fill_rule, &state.current().fill_color, state.current().fill_alpha, transform, pixmap, mask_ref);
    }
    if do_stroke {
        stroke_path(&path, state.current(), transform, pixmap, mask_ref);
    }

    // Apply pending clip if set.
    if let Some(rule) = clip_pending.take() {
        let mut mask = Mask::new(pixmap.width(), pixmap.height())
            .unwrap_or_else(|| Mask::new(1, 1).unwrap());
        mask.fill_path(&path, rule, true, transform);
        *clip_mask = Some(mask);
    }
}

fn apply_pending_clip(
    path_builder: &mut PathBuilder,
    clip_pending: &mut Option<FillRule>,
    clip_mask: &mut Option<Mask>,
    pixmap: &mut Pixmap,
    state: &GraphicsStateStack,
    base: Transform,
) {
    if let Some(rule) = clip_pending.take() {
        let built = std::mem::replace(path_builder, PathBuilder::new());
        if let Some(path) = built.finish() {
            let transform = combined_transform(&state.current().ctm, base);
            let mut mask = Mask::new(pixmap.width(), pixmap.height())
                .unwrap_or_else(|| Mask::new(1, 1).unwrap());
            mask.fill_path(&path, rule, true, transform);
            *clip_mask = Some(mask);
        }
    }
}

fn fill_path(
    path: &Path,
    fill_rule: FillRule,
    color: &Color,
    alpha: f64,
    transform: Transform,
    pixmap: &mut Pixmap,
    mask: Option<&Mask>,
) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(
        (color.r * 255.0) as u8,
        (color.g * 255.0) as u8,
        (color.b * 255.0) as u8,
        (alpha * 255.0) as u8,
    );
    paint.anti_alias = true;
    pixmap.fill_path(path, &paint, fill_rule, transform, mask);
}

fn stroke_path(
    path: &Path,
    gs: &super::graphics::GraphicsState,
    transform: Transform,
    pixmap: &mut Pixmap,
    mask: Option<&Mask>,
) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(
        (gs.stroke_color.r * 255.0) as u8,
        (gs.stroke_color.g * 255.0) as u8,
        (gs.stroke_color.b * 255.0) as u8,
        (gs.stroke_alpha * 255.0) as u8,
    );
    paint.anti_alias = true;

    let mut stroke = Stroke::default();
    stroke.width = gs.line_width as f32;
    stroke.line_cap = match gs.line_cap {
        1 => LineCap::Round,
        2 => LineCap::Square,
        _ => LineCap::Butt,
    };
    stroke.line_join = match gs.line_join {
        1 => LineJoin::Round,
        2 => LineJoin::Bevel,
        _ => LineJoin::Miter,
    };
    stroke.miter_limit = gs.miter_limit as f32;
    if !gs.dash_array.is_empty() {
        let dashes: Vec<f32> = gs.dash_array.iter().map(|&d| d as f32).collect();
        stroke.dash = StrokeDash::new(dashes, gs.dash_phase as f32);
    }
    pixmap.stroke_path(path, &paint, &stroke, transform, mask);
}

/// Resolve fonts from a /Resources dictionary.
fn resolve_fonts_from_resources(
    resources: &PdfObject,
    doc: &PdfDocument,
) -> HashMap<Vec<u8>, ResolvedFont> {
    let mut fonts = HashMap::new();
    let font_dict_ref = match resources.get(b"Font") {
        Some(f) => f,
        None => return fonts,
    };
    let resolved_fonts = match doc.resolve_value(font_dict_ref) {
        Ok(f) => f,
        Err(_) => return fonts,
    };
    let dict = match resolved_fonts.as_dict() {
        Some(d) => d,
        None => return fonts,
    };
    for (name, font_ref) in dict {
        if let Ok(font_obj) = doc.resolve_value(font_ref) {
            if let Ok(font) = super::font::resolve_font(&font_obj, doc) {
                fonts.insert(name.clone(), font);
            }
        }
    }
    fonts
}

/// Render a single positioned glyph. Uses embedded font outlines via ttf-parser
/// when available, falls back to filled rectangles for Standard 14 fonts.
fn render_glyph(
    glyph: &PositionedGlyph,
    fonts: &HashMap<Vec<u8>, ResolvedFont>,
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

    // Try font outline rendering with encoding-aware mapping.
    match fonts.get(&glyph.font_name) {
        Some(ResolvedFont::Embedded { data, first_char, encoding, to_unicode, .. }) => {
            if let Some(path) = glyph_outline_path(data, glyph, *first_char, encoding, to_unicode.as_ref(), 0) {
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
                return;
            }
        }
        Some(ResolvedFont::Standard14 { name, encoding, to_unicode, .. }) => {
            if let Some(font_data) = system_font_for_standard14(name) {
                let face_idx = super::font::ttc_face_index(name);
                if let Some(path) = glyph_outline_path(font_data, glyph, 0, encoding, to_unicode.as_ref(), face_idx) {
                    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
                    return;
                }
            }
        }
        Some(ResolvedFont::CIDFont { data, cid_to_gid, to_unicode, .. }) => {
            let encoding = super::font::Encoding::Identity;
            if let Some(path) = glyph_outline_path(data, glyph, 0, &encoding, to_unicode.as_ref(), 0) {
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
                return;
            }
            // Try CIDToGIDMap-aware resolution.
            if !data.is_empty() {
                let otf_wrapped;
                let face = match ttf_parser::Face::parse(data, 0) {
                    Ok(f) => Some(f),
                    Err(_) => {
                        otf_wrapped = wrap_cff_in_otf(data);
                        ttf_parser::Face::parse(&otf_wrapped, 0).ok()
                    }
                };
                if let Some(face) = face {
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
                            if face.outline_glyph(gid, &mut builder).is_some() {
                                if let Some(path) = builder.pb.finish() {
                                    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, mask);
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }
        None => {}
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

/// Try to build a tiny-skia Path from a font's glyph outline.
///
/// Glyph mapping priority:
/// 0. ToUnicode CMap: char_code → Unicode → face.glyph_index()
/// 1. Encoding Differences: char_code → glyph_name → resolve_glyph_by_name
/// 2. Named encoding (WinAnsi/MacRoman/Standard): char_code → glyph_name → resolve_glyph_by_name
/// 3. cmap: char_code as Unicode → face.glyph_index()
/// 4. Direct glyph index: GlyphId(char_code)
/// 5. FirstChar offset: GlyphId(char_code - first_char)
fn glyph_outline_path(
    font_data: &[u8],
    glyph: &PositionedGlyph,
    first_char: u32,
    encoding: &super::font::Encoding,
    to_unicode: Option<&super::cmap::ToUnicodeMap>,
    face_index: u32,
) -> Option<Path> {
    let otf_wrapped;
    let face = match ttf_parser::Face::parse(font_data, face_index) {
        Ok(f) => f,
        Err(_) => {
            otf_wrapped = wrap_cff_in_otf(font_data);
            match ttf_parser::Face::parse(&otf_wrapped, 0) {
                Ok(f) => f,
                Err(_) => return None,
            }
        }
    };

    let units_per_em = face.units_per_em() as f64;
    if units_per_em == 0.0 {
        return None;
    }

    let code = glyph.char_code;
    let glyph_id = resolve_glyph_id(&face, code, first_char, encoding, to_unicode)?;
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
    encoding: &super::font::Encoding,
    to_unicode: Option<&super::cmap::ToUnicodeMap>,
) -> Option<ttf_parser::GlyphId> {
    use super::encoding::{
        agl_name_to_unicode, macroman_name, resolve_glyph_by_name, standard_name, winansi_name,
    };
    use super::font::Encoding;

    // Strategy 0: ToUnicode CMap (highest priority).
    if let Some(tu) = to_unicode {
        if let Some(unicode_char) = tu.get(code) {
            let gid = face.glyph_index(unicode_char);
            if gid.is_some() && gid != Some(ttf_parser::GlyphId(0)) {
                return gid;
            }
        }
    }

    // Strategy 1: Encoding Differences.
    if let Encoding::Differences { diffs, base, .. } = encoding {
        if let Some(glyph_name) = diffs.get(&(code as u8)) {
            if let Some(gid) = resolve_glyph_by_name(face, glyph_name) {
                return Some(gid);
            }
            if let Some(unicode_char) = agl_name_to_unicode(glyph_name) {
                if let Some(gid) = face.glyph_index(unicode_char) {
                    if gid.0 != 0 { return Some(gid); }
                }
            }
        }
        let name = match base.as_str() {
            "MacRomanEncoding" => macroman_name(code as u8),
            "StandardEncoding" => standard_name(code as u8),
            _ => winansi_name(code as u8),
        };
        if let Some(glyph_name) = name {
            if let Some(gid) = resolve_glyph_by_name(face, glyph_name) {
                return Some(gid);
            }
        }
    }

    // Strategy 2: Named encoding table.
    if let Encoding::Named(enc_name) = encoding {
        let name = match enc_name.as_str() {
            "MacRomanEncoding" => macroman_name(code as u8),
            "StandardEncoding" => standard_name(code as u8),
            _ => winansi_name(code as u8),
        };
        if let Some(glyph_name) = name {
            if let Some(gid) = resolve_glyph_by_name(face, glyph_name) {
                return Some(gid);
            }
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
    if face.outline_glyph(direct_id, &mut NullOutlineBuilder).is_some() {
        return Some(direct_id);
    }

    // Strategy 5: Offset by first_char.
    if code >= first_char {
        let offset_id = ttf_parser::GlyphId((code - first_char) as u16);
        if face.outline_glyph(offset_id, &mut NullOutlineBuilder).is_some() {
            return Some(offset_id);
        }
    }

    None
}

/// Wrap raw CFF (Type1C) data in a minimal OpenType container so ttf-parser can parse it.
/// OpenType/CFF requires: offset table + CFF table record + CFF data.
fn wrap_cff_in_otf(cff_data: &[u8]) -> Vec<u8> {
    let num_tables: u16 = 1;
    let header_size: u32 = 12;
    let record_size: u32 = 16;
    let table_offset = header_size + num_tables as u32 * record_size;

    // Compute CFF table checksum (sum of 32-bit big-endian words, padded).
    let checksum = {
        let mut sum: u32 = 0;
        let mut i = 0;
        while i + 4 <= cff_data.len() {
            sum = sum.wrapping_add(u32::from_be_bytes([
                cff_data[i],
                cff_data[i + 1],
                cff_data[i + 2],
                cff_data[i + 3],
            ]));
            i += 4;
        }
        // Pad remaining bytes
        if i < cff_data.len() {
            let mut tail = [0u8; 4];
            for (j, &b) in cff_data[i..].iter().enumerate() {
                tail[j] = b;
            }
            sum = sum.wrapping_add(u32::from_be_bytes(tail));
        }
        sum
    };

    let mut otf = Vec::with_capacity(table_offset as usize + cff_data.len());

    // Offset table
    otf.extend_from_slice(b"OTTO"); // CFF-based OpenType signature
    otf.extend_from_slice(&num_tables.to_be_bytes());
    otf.extend_from_slice(&16u16.to_be_bytes()); // searchRange
    otf.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
    otf.extend_from_slice(&0u16.to_be_bytes()); // rangeShift

    // CFF table record
    otf.extend_from_slice(b"CFF "); // tag
    otf.extend_from_slice(&checksum.to_be_bytes());
    otf.extend_from_slice(&table_offset.to_be_bytes());
    otf.extend_from_slice(&(cff_data.len() as u32).to_be_bytes());

    // CFF data
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
/// (font Y-up → PDF Y-up, but we apply the base transform later which flips).
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

/// Extract an f64 from operand at index, defaulting to 0.0.
fn arg_f64(args: &[PdfObject], idx: usize) -> f64 {
    args.get(idx).and_then(|o| o.as_f64()).unwrap_or(0.0)
}

/// Extract an f32 from operand at index, defaulting to 0.0.
fn arg_f32(args: &[PdfObject], idx: usize) -> f32 {
    arg_f64(args, idx) as f32
}

/// Cached system font data for Standard 14 font rendering.
/// Keyed by font category: "sans", "serif", "mono".
static SYSTEM_FONT_CACHE: OnceLock<HashMap<&'static str, Vec<u8>>> = OnceLock::new();

/// Try to load a system font matching a Standard 14 font name.
/// Returns cached font data on success, None if no suitable system font found.
fn system_font_for_standard14(name: &str) -> Option<&'static Vec<u8>> {
    let cache = SYSTEM_FONT_CACHE.get_or_init(|| {
        let mut map = HashMap::new();
        // Try sans-serif fonts (Helvetica substitutes)
        for path in SANS_FONT_PATHS {
            if let Ok(data) = std::fs::read(path) {
                map.insert("sans", data);
                break;
            }
        }
        // Try serif fonts (Times substitutes)
        for path in SERIF_FONT_PATHS {
            if let Ok(data) = std::fs::read(path) {
                map.insert("serif", data);
                break;
            }
        }
        // Try monospace fonts (Courier substitutes)
        for path in MONO_FONT_PATHS {
            if let Ok(data) = std::fs::read(path) {
                map.insert("mono", data);
                break;
            }
        }
        map
    });

    let category = match name {
        n if n.starts_with("Courier") => "mono",
        n if n.starts_with("Times") => "serif",
        _ => "sans", // Helvetica and everything else
    };
    cache.get(category)
}

// System font search paths, in priority order.
#[cfg(target_os = "macos")]
const SANS_FONT_PATHS: &[&str] = &[
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/SFNSText.ttf",
    "/Library/Fonts/Arial.ttf",
];
#[cfg(target_os = "macos")]
const SERIF_FONT_PATHS: &[&str] = &[
    "/System/Library/Fonts/Times.ttc",
    "/Library/Fonts/Times New Roman.ttf",
];
#[cfg(target_os = "macos")]
const MONO_FONT_PATHS: &[&str] = &[
    "/System/Library/Fonts/Courier.ttc",
    "/System/Library/Fonts/Menlo.ttc",
    "/Library/Fonts/Courier New.ttf",
];

#[cfg(target_os = "linux")]
const SANS_FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
];
#[cfg(target_os = "linux")]
const SERIF_FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
    "/usr/share/fonts/liberation-serif/LiberationSerif-Regular.ttf",
];
#[cfg(target_os = "linux")]
const MONO_FONT_PATHS: &[&str] = &[
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf",
];

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const SANS_FONT_PATHS: &[&str] = &[];
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const SERIF_FONT_PATHS: &[&str] = &[];
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const MONO_FONT_PATHS: &[&str] = &[];

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::document::PdfDocument;
    use std::io::Write;

    fn make_red_rect_pdf() -> Vec<u8> {
        let content = b"1 0 0 rg 0 0 200 100 re f";
        let mut pdf = Vec::new();
        write!(pdf, "%PDF-1.4\n").unwrap();
        let obj1 = pdf.len();
        write!(pdf, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n").unwrap();
        let obj2 = pdf.len();
        write!(pdf, "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n").unwrap();
        let obj3 = pdf.len();
        write!(pdf, "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>\nendobj\n").unwrap();
        let obj4 = pdf.len();
        write!(pdf, "4 0 obj\n<< /Length {} >>\nstream\n", content.len()).unwrap();
        pdf.extend_from_slice(content);
        write!(pdf, "\nendstream\nendobj\n").unwrap();
        let xref = pdf.len();
        write!(pdf, "xref\n0 5\n").unwrap();
        write!(pdf, "0000000000 65535 f \n").unwrap();
        write!(pdf, "{:010} 00000 n \n", obj1).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj2).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj3).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj4).unwrap();
        write!(pdf, "trailer\n<< /Size 5 /Root 1 0 R >>\n").unwrap();
        write!(pdf, "startxref\n{xref}\n%%EOF\n").unwrap();
        pdf
    }

    #[test]
    fn render_red_rectangle() {
        let data = make_red_rect_pdf();
        let doc = PdfDocument::open(&data).unwrap();
        let pixmap = render_page(&doc, 0, 1.0).unwrap();
        assert_eq!(pixmap.width(), 200);
        assert_eq!(pixmap.height(), 100);

        let rgb = pixmap_to_rgb8(&pixmap);
        let cx = 100;
        let cy = 50;
        let idx = (cy * 200 + cx) * 3;
        assert!(rgb[idx] > 200, "Red should be high, got {}", rgb[idx]);
        assert!(rgb[idx + 1] < 50, "Green should be low, got {}", rgb[idx + 1]);
        assert!(rgb[idx + 2] < 50, "Blue should be low, got {}", rgb[idx + 2]);
    }

    #[test]
    fn render_at_2x_scale() {
        let data = make_red_rect_pdf();
        let doc = PdfDocument::open(&data).unwrap();
        let pixmap = render_page(&doc, 0, 2.0).unwrap();
        assert_eq!(pixmap.width(), 400);
        assert_eq!(pixmap.height(), 200);
    }

    #[test]
    fn pixmap_to_rgb8_white_bg() {
        let pixmap = Pixmap::new(1, 1).unwrap();
        let rgb = pixmap_to_rgb8(&pixmap);
        assert_eq!(rgb, vec![255, 255, 255]);
    }

    #[test]
    fn pixmap_to_rgb8_opaque() {
        let mut pixmap = Pixmap::new(1, 1).unwrap();
        pixmap.data_mut()[0] = 255;
        pixmap.data_mut()[1] = 0;
        pixmap.data_mut()[2] = 0;
        pixmap.data_mut()[3] = 255;
        let rgb = pixmap_to_rgb8(&pixmap);
        assert_eq!(rgb, vec![255, 0, 0]);
    }

    #[test]
    fn render_gray_fill() {
        let content = b"0.5 g 0 0 100 100 re f";
        let mut pdf = Vec::new();
        write!(pdf, "%PDF-1.4\n").unwrap();
        let obj1 = pdf.len();
        write!(pdf, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n").unwrap();
        let obj2 = pdf.len();
        write!(pdf, "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n").unwrap();
        let obj3 = pdf.len();
        write!(pdf, "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>\nendobj\n").unwrap();
        let obj4 = pdf.len();
        write!(pdf, "4 0 obj\n<< /Length {} >>\nstream\n", content.len()).unwrap();
        pdf.extend_from_slice(content);
        write!(pdf, "\nendstream\nendobj\n").unwrap();
        let xref = pdf.len();
        write!(pdf, "xref\n0 5\n").unwrap();
        write!(pdf, "0000000000 65535 f \n").unwrap();
        write!(pdf, "{:010} 00000 n \n", obj1).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj2).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj3).unwrap();
        write!(pdf, "{:010} 00000 n \n", obj4).unwrap();
        write!(pdf, "trailer\n<< /Size 5 /Root 1 0 R >>\n").unwrap();
        write!(pdf, "startxref\n{xref}\n%%EOF\n").unwrap();

        let doc = PdfDocument::open(&pdf).unwrap();
        let pixmap = render_page(&doc, 0, 1.0).unwrap();
        let rgb = pixmap_to_rgb8(&pixmap);
        let idx = (50 * 100 + 50) * 3;
        assert!((rgb[idx] as i32 - 128).abs() < 5, "Got {}", rgb[idx]);
        assert!((rgb[idx + 1] as i32 - 128).abs() < 5);
        assert!((rgb[idx + 2] as i32 - 128).abs() < 5);
    }
}
