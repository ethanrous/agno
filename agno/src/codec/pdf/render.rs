//! PDF page rasterizer using tiny-skia.
//!
//! Converts parsed PDF content streams into RGBA pixels via the tiny-skia 2D
//! rendering library, then composites over a white background to produce RGB8.

use std::collections::HashMap;
use std::error::Error;

use tiny_skia::{
    FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, Stroke,
    StrokeDash, Transform,
};

use super::content::{parse_content_stream, Operator};
use super::document::PdfDocument;
use super::font::ResolvedFont;
use super::glyph::{build_font_data_cache, render_glyph};
use super::graphics::{Color, GraphicsStateStack, Matrix};
use super::objects::PdfObject;
use super::text::layout_text;

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

    const MAX_PIXEL_COUNT: u64 = 100_000_000;
    if (render_w as u64) * (render_h as u64) > MAX_PIXEL_COUNT {
        return Err(format!(
            "Page dimensions too large: {}x{} ({} pixels exceeds limit of {})",
            render_w,
            render_h,
            (render_w as u64) * (render_h as u64),
            MAX_PIXEL_COUNT
        )
        .into());
    }

    let mut pixmap = Pixmap::new(render_w, render_h)
        .ok_or("Failed to create pixmap (dimensions too large)")?;
    pixmap.fill(tiny_skia::Color::WHITE);

    let content = doc.page_content_stream(page_index)?;
    let operators = parse_content_stream(&content)?;

    let resources = doc.page_resources(page_index).unwrap_or_else(|_| {
        PdfObject::Dictionary(HashMap::new())
    });
    let fonts = resolve_fonts_from_resources(&resources, doc);
    let font_data_cache = build_font_data_cache(&fonts);

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

    execute_content_stream(&operators, &mut state, &mut pixmap, &ctx, &fonts, &font_data_cache, clip_mask.as_ref());

    // Downscale from render resolution to target resolution.
    if render_w != target_w || render_h != target_h {
        Ok(downscale_pixmap(&pixmap, target_w, target_h)?)
    } else {
        Ok(pixmap)
    }
}

/// Downscale a pixmap using box-filter averaging (equivalent to area-based downsampling).
/// This produces high-quality anti-aliased output from a supersampled source.
fn downscale_pixmap(src: &Pixmap, dst_w: u32, dst_h: u32) -> Result<Pixmap, Box<dyn Error>> {
    let mut dst = Pixmap::new(dst_w, dst_h)
        .ok_or("Failed to allocate downscale pixmap")?;
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
    Ok(dst)
}

/// Execute a parsed content stream, handling BT/ET text blocks and dispatching
/// individual operators. Shared by `render_page` and `render_xobject`.
fn execute_content_stream(
    operators: &[Operator],
    state: &mut GraphicsStateStack,
    pixmap: &mut Pixmap,
    ctx: &RenderContext,
    fonts: &HashMap<Vec<u8>, ResolvedFont>,
    font_data_cache: &HashMap<Vec<u8>, Vec<u8>>,
    initial_clip: Option<&Mask>,
) {
    let mut path_builder = PathBuilder::new();
    let mut clip_pending: Option<FillRule> = None;
    let mut clip_mask: Option<Mask> = initial_clip.cloned();
    let mut current_point: Option<(f32, f32)> = None;
    let mut subpath_start: Option<(f32, f32)> = None;

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
                            &mut current_point,
                            &mut subpath_start,
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
                    | b"Tz" | b"Ts" | b"Tr"
                    | b"'" | b"\""
                ))
                .collect();

            if !text_ops.is_empty() {
                if let Ok(glyphs) = layout_text(&text_ops, state.current(), fonts) {
                    let transform = combined_transform(&state.current().ctm, ctx.base);
                    let color = &state.current().fill_color;
                    let alpha = state.current().fill_alpha;

                    let mask_ref = clip_mask.as_ref();
                    for glyph in &glyphs {
                        render_glyph(
                            glyph,
                            fonts,
                            font_data_cache,
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
                &mut current_point,
                &mut subpath_start,
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
    current_point: &mut Option<(f32, f32)>,
    subpath_start: &mut Option<(f32, f32)>,
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
            let (x, y) = (arg_f32(args, 0), arg_f32(args, 1));
            path_builder.move_to(x, y);
            *current_point = Some((x, y));
            *subpath_start = Some((x, y));
        }
        b"l" if args.len() >= 2 => {
            let (x, y) = (arg_f32(args, 0), arg_f32(args, 1));
            path_builder.line_to(x, y);
            *current_point = Some((x, y));
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
            *current_point = Some((arg_f32(args, 4), arg_f32(args, 5)));
        }
        b"v" if args.len() >= 4 => {
            let (cpx, cpy) = current_point.unwrap_or((0.0, 0.0));
            path_builder.cubic_to(
                cpx, cpy,
                arg_f32(args, 0), arg_f32(args, 1),
                arg_f32(args, 2), arg_f32(args, 3),
            );
            *current_point = Some((arg_f32(args, 2), arg_f32(args, 3)));
        }
        b"y" if args.len() >= 4 => {
            path_builder.cubic_to(
                arg_f32(args, 0), arg_f32(args, 1),
                arg_f32(args, 2), arg_f32(args, 3),
                arg_f32(args, 2), arg_f32(args, 3),
            );
            *current_point = Some((arg_f32(args, 2), arg_f32(args, 3)));
        }
        b"h" => {
            path_builder.close();
            *current_point = *subpath_start;
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
            *current_point = Some((x, y));
            *subpath_start = Some((x, y));
        }

        // --- Path painting ---
        b"S" => {
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, false, true, FillRule::Winding);
            *current_point = None;
            *subpath_start = None;
        }
        b"s" => {
            path_builder.close();
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, false, true, FillRule::Winding);
            *current_point = None;
            *subpath_start = None;
        }
        b"f" | b"F" => {
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, true, false, FillRule::Winding);
            *current_point = None;
            *subpath_start = None;
        }
        b"f*" => {
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, true, false, FillRule::EvenOdd);
            *current_point = None;
            *subpath_start = None;
        }
        b"B" => {
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, true, true, FillRule::Winding);
            *current_point = None;
            *subpath_start = None;
        }
        b"B*" => {
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, true, true, FillRule::EvenOdd);
            *current_point = None;
            *subpath_start = None;
        }
        b"b" => {
            path_builder.close();
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, true, true, FillRule::Winding);
            *current_point = None;
            *subpath_start = None;
        }
        b"b*" => {
            path_builder.close();
            paint_path(state, pixmap, ctx.base, path_builder, clip_pending, clip_mask, true, true, FillRule::EvenOdd);
            *current_point = None;
            *subpath_start = None;
        }
        b"n" => {
            apply_pending_clip(path_builder, clip_pending, clip_mask, pixmap, state, ctx.base);
            *path_builder = PathBuilder::new();
            *current_point = None;
            *subpath_start = None;
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
        b"gs" if !args.is_empty() => {
            if let Some(gs_name) = args[0].as_name() {
                if let Some(ext_gs) = ctx.resources.get(b"ExtGState") {
                    if let Ok(gs_dict) = ctx.doc.resolve_value(ext_gs) {
                        if let Some(entry) = gs_dict.get(gs_name) {
                            if let Ok(gs_obj) = ctx.doc.resolve_value(entry) {
                                if let Some(ca) = gs_obj.get_f64(b"ca") {
                                    state.current_mut().fill_alpha = ca.clamp(0.0, 1.0);
                                }
                                if let Some(big_ca) = gs_obj.get_f64(b"CA") {
                                    state.current_mut().stroke_alpha = big_ca.clamp(0.0, 1.0);
                                }
                            }
                        }
                    }
                }
            }
        }
        b"gs" => {}

        // --- Inline image ---
        b"BI" if op.operands.len() >= 2 => {
            render_inline_image(&op.operands[0], &op.operands[1], state, pixmap, ctx);
        }
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
fn render_inline_image(
    dict: &PdfObject,
    data_obj: &PdfObject,
    state: &GraphicsStateStack,
    pixmap: &mut Pixmap,
    ctx: &RenderContext,
) {
    let raw_data = match data_obj.as_string_bytes() {
        Some(d) => d,
        None => return,
    };

    let img_w = dict.get_i64(b"Width").unwrap_or(0) as u32;
    let img_h = dict.get_i64(b"Height").unwrap_or(0) as u32;
    if img_w == 0 || img_h == 0 { return; }

    let bpc = dict.get_i64(b"BitsPerComponent").unwrap_or(1) as u8;
    let is_mask = dict.get(b"ImageMask")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let cs = if is_mask {
        super::color::ColorSpace::DeviceGray
    } else {
        dict.get(b"ColorSpace")
            .and_then(|cs| parse_image_colorspace(cs, ctx.doc))
            .unwrap_or(super::color::ColorSpace::DeviceGray)
    };

    let filter = dict.get(b"Filter").and_then(|f| f.as_name()).map(|n| n.to_vec());
    let filter_ref = filter.as_deref();

    // For image masks, render as 1-bit black/white using current fill color.
    if is_mask && bpc == 1 {
        render_inline_mask(raw_data, img_w, img_h, state, pixmap, ctx);
        return;
    }

    let image = match super::image::decode_image_xobject(
        raw_data, img_w, img_h, bpc, &cs, filter_ref,
    ) {
        Ok(img) => img,
        Err(_) => return,
    };

    let mut rgba = Vec::with_capacity(image.width as usize * image.height as usize * 4);
    for pixel in image.rgb_data.chunks(3) {
        if pixel.len() < 3 { break; }
        rgba.push(pixel[0]);
        rgba.push(pixel[1]);
        rgba.push(pixel[2]);
        rgba.push(255);
    }

    let src_pixmap = match tiny_skia::PixmapRef::from_bytes(&rgba, image.width, image.height) {
        Some(p) => p,
        None => return,
    };

    let transform = image_transform(state, ctx, image.width, image.height);
    let paint = tiny_skia::PixmapPaint {
        opacity: 1.0,
        blend_mode: tiny_skia::BlendMode::SourceOver,
        quality: tiny_skia::FilterQuality::Bilinear,
    };
    pixmap.draw_pixmap(0, 0, src_pixmap, &paint, transform, None);
}

/// Render a 1-bit image mask: bit=0 paints fill color, bit=1 is transparent.
fn render_inline_mask(
    data: &[u8],
    img_w: u32,
    img_h: u32,
    state: &GraphicsStateStack,
    pixmap: &mut Pixmap,
    ctx: &RenderContext,
) {
    let color = &state.current().fill_color;
    let r = (color.r.clamp(0.0, 1.0) * 255.0) as u8;
    let g = (color.g.clamp(0.0, 1.0) * 255.0) as u8;
    let b = (color.b.clamp(0.0, 1.0) * 255.0) as u8;

    let mut rgba = vec![0u8; img_w as usize * img_h as usize * 4];
    let row_bytes = (img_w as usize + 7) / 8;

    for y in 0..img_h as usize {
        for x in 0..img_w as usize {
            let byte_idx = y * row_bytes + x / 8;
            let bit_idx = 7 - (x % 8);
            let bit = if byte_idx < data.len() {
                (data[byte_idx] >> bit_idx) & 1
            } else { 1 };

            let px = (y * img_w as usize + x) * 4;
            if bit == 0 {
                // Bit 0 = paint with fill color
                rgba[px] = r;
                rgba[px + 1] = g;
                rgba[px + 2] = b;
                rgba[px + 3] = 255;
            }
            // Bit 1 = transparent (already 0,0,0,0)
        }
    }

    let src_pixmap = match tiny_skia::PixmapRef::from_bytes(&rgba, img_w, img_h) {
        Some(p) => p,
        None => return,
    };

    let transform = image_transform(state, ctx, img_w, img_h);
    let paint = tiny_skia::PixmapPaint {
        opacity: 1.0,
        blend_mode: tiny_skia::BlendMode::SourceOver,
        quality: tiny_skia::FilterQuality::Nearest,
    };
    pixmap.draw_pixmap(0, 0, src_pixmap, &paint, transform, None);
}

/// Build the transform for rendering an image into the PDF coordinate system.
/// PDF images occupy a 1x1 unit square with origin at bottom-left, but pixel
/// data has row 0 at top. Flip Y to compensate.
fn image_transform(
    state: &GraphicsStateStack,
    ctx: &RenderContext,
    img_w: u32,
    img_h: u32,
) -> Transform {
    let ctm = &state.current().ctm;
    let w = img_w as f32;
    let h = img_h as f32;
    // Scale pixel coords to 1x1, then flip Y (negate c,d and offset by c,d).
    let t = Transform::from_row(
        ctm.a as f32 / w,
        ctm.b as f32 / w,
        -(ctm.c as f32) / h,
        -(ctm.d as f32) / h,
        (ctm.e + ctm.c) as f32,
        (ctm.f + ctm.d) as f32,
    );
    ctx.base.pre_concat(t)
}

fn render_image_xobject(
    xobj: &PdfObject,
    state: &GraphicsStateStack,
    pixmap: &mut Pixmap,
    ctx: &RenderContext,
) {
    let (_, stream_data) = match xobj.as_stream() {
        Some(sd) => sd,
        None => return,
    };

    let img_w = xobj.get_i64(b"Width").unwrap_or(0) as u32;
    let img_h = xobj.get_i64(b"Height").unwrap_or(0) as u32;
    if img_w == 0 || img_h == 0 { return; }

    let bpc = xobj.get_i64(b"BitsPerComponent").unwrap_or(8) as u8;

    // Determine color space.
    let cs = xobj.get(b"ColorSpace")
        .and_then(|cs| parse_image_colorspace(cs, ctx.doc))
        .unwrap_or(super::color::ColorSpace::DeviceRGB);

    // Determine if the raw data is JPEG (DCTDecode passes through stream.rs).
    let filter = xobj.get(b"Filter")
        .and_then(|f| f.as_name())
        .map(|n| n.to_vec());
    let filter_ref = filter.as_deref();

    let image = match super::image::decode_image_xobject(
        stream_data, img_w, img_h, bpc, &cs, filter_ref,
    ) {
        Ok(img) => img,
        Err(_) => return,
    };

    // Convert RGB8 to RGBA8 for tiny-skia PixmapRef.
    let mut rgba = Vec::with_capacity(image.width as usize * image.height as usize * 4);
    for pixel in image.rgb_data.chunks(3) {
        if pixel.len() < 3 { break; }
        rgba.push(pixel[0]);
        rgba.push(pixel[1]);
        rgba.push(pixel[2]);
        rgba.push(255);
    }

    let src_pixmap = match tiny_skia::PixmapRef::from_bytes(&rgba, image.width, image.height) {
        Some(p) => p,
        None => return,
    };

    let transform = image_transform(state, ctx, image.width, image.height);
    let paint = tiny_skia::PixmapPaint {
        opacity: state.current().fill_alpha as f32,
        blend_mode: tiny_skia::BlendMode::SourceOver,
        quality: tiny_skia::FilterQuality::Bilinear,
    };
    pixmap.draw_pixmap(0, 0, src_pixmap, &paint, transform, None);
}

fn parse_image_colorspace(
    cs_obj: &PdfObject,
    doc: &PdfDocument,
) -> Option<super::color::ColorSpace> {
    if let Some(name) = cs_obj.as_name_str() {
        return match name {
            "DeviceRGB" | "RGB" => Some(super::color::ColorSpace::DeviceRGB),
            "DeviceGray" | "G" => Some(super::color::ColorSpace::DeviceGray),
            "DeviceCMYK" | "CMYK" => Some(super::color::ColorSpace::DeviceCMYK),
            _ => None,
        };
    }
    if let Some(arr) = cs_obj.as_array() {
        if let Some(name) = arr.first().and_then(|v| v.as_name_str()) {
            return match name {
                "ICCBased" => {
                    let n = arr.get(1)
                        .and_then(|r| doc.resolve_value(r).ok())
                        .and_then(|o| o.get_i64(b"N"))
                        .unwrap_or(3) as u8;
                    Some(super::color::ColorSpace::ICCBased { num_components: n })
                }
                "CalRGB" => Some(super::color::ColorSpace::CalRGB),
                "CalGray" => Some(super::color::ColorSpace::CalGray),
                "DeviceRGB" => Some(super::color::ColorSpace::DeviceRGB),
                "DeviceGray" => Some(super::color::ColorSpace::DeviceGray),
                "DeviceCMYK" => Some(super::color::ColorSpace::DeviceCMYK),
                _ => None,
            };
        }
    }
    None
}

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
    if subtype == "Image" {
        render_image_xobject(&xobj, state, pixmap, ctx);
        return;
    }

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
    let form_font_cache = build_font_data_cache(&form_fonts);

    // 7. Build child context with incremented depth
    let child_ctx = RenderContext {
        doc: ctx.doc,
        resources: form_resources,
        base: ctx.base,
        depth: ctx.depth + 1,
    };

    // 8. Execute the form's content stream
    execute_content_stream(&content_ops, state, pixmap, &child_ctx, &form_fonts, &form_font_cache, clip);

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
        if let Some(mut mask) = Mask::new(pixmap.width(), pixmap.height()) {
            mask.fill_path(&path, rule, true, transform);
            *clip_mask = Some(mask);
        }
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
            if let Some(mut mask) = Mask::new(pixmap.width(), pixmap.height()) {
                mask.fill_path(&path, rule, true, transform);
                *clip_mask = Some(mask);
            }
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

/// Extract an f64 from operand at index, defaulting to 0.0.
fn arg_f64(args: &[PdfObject], idx: usize) -> f64 {
    args.get(idx).and_then(|o| o.as_f64()).unwrap_or(0.0)
}

/// Extract an f32 from operand at index, defaulting to 0.0.
fn arg_f32(args: &[PdfObject], idx: usize) -> f32 {
    arg_f64(args, idx) as f32
}

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
