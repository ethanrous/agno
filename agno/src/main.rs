use std::env;
use std::error::Error;
use std::fs::File;

use agno::agno_image::load::load_agno_image_from_file;
use agno::agno_image::transform::{PadColor, scale_image};
use agno::exif::{ExifContext, ExifValue};
use agno::logging::{LogConfig, init};

const AGNO_BUILD_VERSION: Option<&str> = option_env!("AGNO_BUILD_VERSION");

fn main() -> Result<(), Box<dyn Error>> {
    init(LogConfig::cli());
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <command> [args...]", args[0]);
        eprintln!("Commands:");
        eprintln!("  exif <file>                              Print EXIF data for a file");
        eprintln!("  convert <input> <output>                 Convert image between formats");
        eprintln!(
            "  resize [--no-stretch] [--pad-color RRGGBB|RRGGBBAA] <input> <width> <height> <output>"
        );
        eprintln!(
            "                                           Resize image; --no-stretch preserves aspect ratio and pads"
        );
        eprintln!("  --version                                Print version information");
        return Ok(());
    }

    match args[1].as_str() {
        "exif" => cmd_exif(&args[2..]),
        "convert" => cmd_convert(&args[2..]),
        "resize" => cmd_resize(&args[2..]),
        "--version" => {
            println!("AGNO {}", AGNO_BUILD_VERSION.unwrap_or("devel"));
            Ok(())
        }
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            Err("Unknown command".into())
        }
    }
}

fn cmd_exif(args: &[String]) -> Result<(), Box<dyn Error>> {
    if args.is_empty() {
        eprintln!("Usage: agno exif <file>");
        return Err("No file path provided".into());
    }

    let path = &args[0];
    let mut file = File::open(path)?;
    let ctx = ExifContext::from_reader_auto(&mut file)?;

    // Print all EXIF values sorted by tag
    let mut entries: Vec<_> = ctx.iter_all().collect();
    entries.sort_by_key(|(_, tag, _)| *tag);

    for (name, tag, value) in entries {
        print_exif_value(name, tag, value);
    }

    Ok(())
}

fn print_exif_value(name: &str, tag: u16, value: &ExifValue) {
    if name == "Unknown" {
        return;
    }

    match value {
        ExifValue::Ascii(s) => println!("{:<35} (0x{:04x}): {}", name, tag, s),
        ExifValue::Short(v) if v.len() == 1 => {
            // Display as signed if the value looks negative (>= 32768)
            if v[0] >= 32768 {
                println!("{:<35} (0x{:04x}): {}", name, tag, v[0] as i16)
            } else {
                println!("{:<35} (0x{:04x}): {}", name, tag, v[0])
            }
        }
        ExifValue::Short(v) => {
            // If any value >= 32768, display all as signed
            let has_negative = v.iter().any(|&x| x >= 32768);
            if has_negative {
                let signed: Vec<i16> = v.iter().map(|&x| x as i16).collect();
                println!("{:<35} (0x{:04x}): {:?}", name, tag, signed);
            } else {
                println!("{:<35} (0x{:04x}): {:?}", name, tag, v);
            }
        }
        ExifValue::Long(v) if v.len() == 1 => println!("{:<35} (0x{:04x}): {}", name, tag, v[0]),
        ExifValue::Long(v) => println!("{:<35} (0x{:04x}): {:?}", name, tag, v),
        ExifValue::Rational(v) if v.len() == 1 => {
            println!("{:<35} (0x{:04x}): {}/{}", name, tag, v[0].0, v[0].1)
        }
        ExifValue::Rational(v) => {
            let ratios: Vec<String> = v.iter().map(|(n, d)| format!("{}/{}", n, d)).collect();
            println!("{:<35} (0x{:04x}): {}", name, tag, ratios.join(" "));
        }
        ExifValue::SLong(v) if v.len() == 1 => println!("{:<35} (0x{:04x}): {}", name, tag, v[0]),
        ExifValue::SLong(v) => println!("{:<35} (0x{:04x}): {:?}", name, tag, v),
        ExifValue::SRational(v) if v.len() == 1 => {
            println!("{:<35} (0x{:04x}): {}/{}", name, tag, v[0].0, v[0].1)
        }
        ExifValue::SRational(v) => {
            // For color matrix, show simplified values
            let values: Vec<String> = v
                .iter()
                .map(|(n, d)| {
                    if *d == 1 {
                        format!("{}", n)
                    } else if *d != 0 {
                        format!("{}", n / d)
                    } else {
                        format!("{}/{}", n, d)
                    }
                })
                .collect();
            println!("{:<35} (0x{:04x}): {}", name, tag, values.join(" "));
        }
        ExifValue::Byte(v) if v.len() <= 16 => {
            println!("{:<35} (0x{:04x}): {:?}", name, tag, v)
        }
        ExifValue::Byte(_) => {
            println!("{:<35} (0x{:04x}): <binary data hidden>", name, tag)
        }
    }
}

/// Parse a page spec: "all", "3", "3-6", "1,3,5", or combinations like "1-3,7".
/// Returns 0-based page indices.
fn parse_page_spec(spec: &str, page_count: usize) -> Result<Vec<usize>, Box<dyn Error>> {
    if page_count == 0 {
        return Err("Document has no pages".into());
    }
    if spec == "all" {
        return Ok((0..page_count).collect());
    }

    let mut pages = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if let Some((start, end)) = part.split_once('-') {
            let s: usize = start
                .trim()
                .parse()
                .map_err(|_| format!("Invalid page: {start}"))?;
            let e: usize = end
                .trim()
                .parse()
                .map_err(|_| format!("Invalid page: {end}"))?;
            if s == 0 || e == 0 {
                return Err("Pages are 1-based".into());
            }
            if s > e {
                return Err(format!("Invalid range: {s}-{e}").into());
            }
            for p in s..=e {
                if p > page_count {
                    return Err(
                        format!("Page {p} out of range (document has {page_count} pages)").into(),
                    );
                }
                pages.push(p - 1);
            }
        } else {
            let n: usize = part.parse().map_err(|_| format!("Invalid page: {part}"))?;
            if n == 0 {
                return Err("Pages are 1-based".into());
            }
            if n > page_count {
                return Err(
                    format!("Page {n} out of range (document has {page_count} pages)").into(),
                );
            }
            pages.push(n - 1);
        }
    }

    if pages.is_empty() {
        return Err("No pages specified".into());
    }
    Ok(pages)
}

/// Stack multiple `AgnoImage`s vertically into a single image. Wider rows are padded
/// with white on the right. Used by both PDF and GIF page export.
#[cfg(feature = "jpeg")]
fn stack_pages_vertically(
    pages: Vec<agno::agno_image::AgnoImage>,
) -> Result<agno::agno_image::AgnoImage, Box<dyn Error>> {
    use agno::agno_image::AgnoImage;
    use agno::exif::ExifContext;

    if pages.is_empty() {
        return Err("stack_pages_vertically: no pages provided".into());
    }
    if pages.len() == 1 {
        return Ok(pages.into_iter().next().unwrap());
    }

    let max_w = pages.iter().map(|p| p.width).max().unwrap();
    let total_h: u64 = pages.iter().map(|p| p.height).sum();

    let buf_size = (max_w as usize)
        .checked_mul(total_h as usize)
        .and_then(|n| n.checked_mul(3))
        .ok_or("Combined image dimensions overflow")?;
    let mut combined = vec![255u8; buf_size];
    let mut y_offset: u64 = 0;
    for page_img in &pages {
        let src = page_img.as_slice();
        let page_row_bytes_u64 = page_img
            .width
            .checked_mul(3)
            .ok_or("Page row size overflow")?;
        let combined_row_bytes_u64 = max_w.checked_mul(3).ok_or("Combined row size overflow")?;
        let copy_len = usize::try_from(page_row_bytes_u64).map_err(|_| "Page row size overflow")?;

        for row in 0..page_img.height {
            let src_start_u64 = row
                .checked_mul(page_row_bytes_u64)
                .ok_or("Source offset overflow")?;
            let src_end_u64 = src_start_u64
                .checked_add(page_row_bytes_u64)
                .ok_or("Source end overflow")?;
            let dst_row_u64 = y_offset
                .checked_add(row)
                .ok_or("Destination row overflow")?;
            let dst_start_u64 = dst_row_u64
                .checked_mul(combined_row_bytes_u64)
                .ok_or("Destination offset overflow")?;

            let src_start = usize::try_from(src_start_u64).map_err(|_| "Source offset overflow")?;
            let src_end = usize::try_from(src_end_u64).map_err(|_| "Source end overflow")?;
            let dst_start =
                usize::try_from(dst_start_u64).map_err(|_| "Destination offset overflow")?;
            let dst_end = dst_start
                .checked_add(copy_len)
                .ok_or("Destination end overflow")?;

            combined[dst_start..dst_end].copy_from_slice(&src[src_start..src_end]);
        }
        y_offset = y_offset
            .checked_add(page_img.height)
            .ok_or("Combined image height overflow")?;
    }

    Ok(AgnoImage::new(
        combined,
        max_w,
        total_h,
        ExifContext::default(),
    ))
}

#[cfg(feature = "jpeg")]
fn cmd_convert(args: &[String]) -> Result<(), Box<dyn Error>> {
    if args.len() < 2 {
        eprintln!("Usage: agno convert [options] <input> <output>");
        eprintln!("Options:");
        eprintln!("  --page SPEC    Page selection for PDF/GIF (default: 1)");
        eprintln!("                 Examples: --page 2, --page 1-3, --page 1,3,5, --page all");
        eprintln!("                 Multiple pages are stacked vertically in the output");
        return Err("Missing input or output path".into());
    }

    let mut page_spec: Option<String> = None;
    let mut positional = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--page" => {
                i += 1;
                page_spec = Some(args.get(i).ok_or("--page requires a value")?.clone());
            }
            "--all-pages" => page_spec = Some("all".to_string()),
            _ => positional.push(args[i].as_str()),
        }
        i += 1;
    }

    if positional.len() < 2 {
        return Err("Missing input or output path".into());
    }
    let input_path = positional[0];
    let output_path = positional[1];

    let extension = std::path::Path::new(input_path)
        .extension()
        .and_then(|ext| ext.to_str());

    let mut header = [0u8; 1024];
    let header_len = std::fs::File::open(input_path)
        .ok()
        .and_then(|mut f| {
            use std::io::Read;
            f.read(&mut header).ok()
        })
        .unwrap_or(0);

    let is_pdf = header[..header_len]
        .windows(5)
        .any(|window| window == b"%PDF-")
        || extension.is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    let is_gif = (header_len >= 6 && (&header[..6] == b"GIF87a" || &header[..6] == b"GIF89a"))
        || extension.is_some_and(|ext| ext.eq_ignore_ascii_case("gif"));

    if is_pdf && page_spec.is_some() {
        convert_pdf(input_path, output_path, page_spec.as_deref().unwrap())?;
    } else if is_gif && page_spec.is_some() {
        convert_gif(input_path, output_path, page_spec.as_deref().unwrap())?;
    } else {
        let img = load_agno_image_from_file(input_path)?;
        img.write_to_file(output_path, 100)?;
    }

    println!("Converted {} -> {}", input_path, output_path);
    Ok(())
}

#[cfg(all(feature = "jpeg", feature = "pdf"))]
fn convert_pdf(input: &str, output: &str, page_spec: &str) -> Result<(), Box<dyn Error>> {
    use agno::exif::ExifContext;

    let data = std::fs::read(input)?;
    let page_count = agno::codec::pdf::pdf_page_count(&data)?;
    let page_indices = parse_page_spec(page_spec, page_count)?;

    if page_indices.len() == 1 {
        let p = page_indices[0];
        let img = agno::agno_image::load::load_pdf_page_from_bytes(
            &data,
            p,
            None,
            ExifContext::default(),
        )?;
        img.write_to_file(output, 100)?;
        println!("Rendered page {} of {page_count}", p + 1);
    } else {
        let mut pages = Vec::with_capacity(page_indices.len());
        for &p in &page_indices {
            pages.push(agno::agno_image::load::load_pdf_page_from_bytes(
                &data,
                p,
                None,
                ExifContext::default(),
            )?);
        }
        let combined = stack_pages_vertically(pages)?;
        let max_w = combined.width;
        let total_h = combined.height;
        combined.write_to_file(output, 100)?;
        println!("Rendered {} pages ({max_w}x{total_h})", page_indices.len());
    }
    Ok(())
}

#[cfg(all(feature = "jpeg", not(feature = "pdf")))]
fn convert_pdf(_input: &str, _output: &str, _page_spec: &str) -> Result<(), Box<dyn Error>> {
    Err("PDF support requires the 'pdf' feature.".into())
}

#[cfg(all(feature = "jpeg", feature = "gif"))]
fn convert_gif(input: &str, output: &str, page_spec: &str) -> Result<(), Box<dyn Error>> {
    use agno::exif::ExifContext;

    let data = std::fs::read(input)?;
    let frame_count = agno::codec::gif::gif_frame_count(&data)?;
    let frame_indices = parse_page_spec(page_spec, frame_count)?;

    if frame_indices.len() == 1 {
        let f = frame_indices[0];
        let img =
            agno::agno_image::load::load_gif_frame_from_bytes(&data, f, ExifContext::default())?;
        img.write_to_file(output, 100)?;
        println!("Rendered frame {} of {frame_count}", f + 1);
    } else {
        let mut pages = Vec::with_capacity(frame_indices.len());
        for &f in &frame_indices {
            pages.push(agno::agno_image::load::load_gif_frame_from_bytes(
                &data,
                f,
                ExifContext::default(),
            )?);
        }
        let combined = stack_pages_vertically(pages)?;
        let max_w = combined.width;
        let total_h = combined.height;
        combined.write_to_file(output, 100)?;
        println!(
            "Rendered {} frames ({max_w}x{total_h})",
            frame_indices.len()
        );
    }
    Ok(())
}

#[cfg(all(feature = "jpeg", not(feature = "gif")))]
fn convert_gif(_input: &str, _output: &str, _page_spec: &str) -> Result<(), Box<dyn Error>> {
    Err("GIF support requires the 'gif' feature.".into())
}

#[cfg(not(feature = "jpeg"))]
fn cmd_convert(_args: &[String]) -> Result<(), Box<dyn Error>> {
    Err("Convert command requires the 'jpeg' feature.".into())
}

/// Parse a hex color of 6 (`RRGGBB`) or 8 (`RRGGBBAA`) digits, with optional
/// `0x` / `#` prefix. Returns `PadColor::Rgb` or `PadColor::Rgba` accordingly.
fn parse_pad_color(s: &str) -> Result<PadColor, Box<dyn Error>> {
    let trimmed = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .or_else(|| s.strip_prefix('#'))
        .unwrap_or(s);
    match trimmed.len() {
        6 => {
            let r = u8::from_str_radix(&trimmed[0..2], 16)
                .map_err(|_| format!("--pad-color: invalid hex in {:?}", s))?;
            let g = u8::from_str_radix(&trimmed[2..4], 16)
                .map_err(|_| format!("--pad-color: invalid hex in {:?}", s))?;
            let b = u8::from_str_radix(&trimmed[4..6], 16)
                .map_err(|_| format!("--pad-color: invalid hex in {:?}", s))?;
            Ok(PadColor::Rgb([r, g, b]))
        }
        8 => {
            let r = u8::from_str_radix(&trimmed[0..2], 16)
                .map_err(|_| format!("--pad-color: invalid hex in {:?}", s))?;
            let g = u8::from_str_radix(&trimmed[2..4], 16)
                .map_err(|_| format!("--pad-color: invalid hex in {:?}", s))?;
            let b = u8::from_str_radix(&trimmed[4..6], 16)
                .map_err(|_| format!("--pad-color: invalid hex in {:?}", s))?;
            let a = u8::from_str_radix(&trimmed[6..8], 16)
                .map_err(|_| format!("--pad-color: invalid hex in {:?}", s))?;
            Ok(PadColor::Rgba([r, g, b, a]))
        }
        n => Err(format!(
            "--pad-color expects 6 (RRGGBB) or 8 (RRGGBBAA) hex digits, got {n} in {:?}",
            s
        )
        .into()),
    }
}

#[cfg(feature = "jpeg")]
fn cmd_resize(args: &[String]) -> Result<(), Box<dyn Error>> {
    use agno::agno_image::transform::scale_image_no_stretch;

    let mut no_stretch = false;
    let mut pad_color: PadColor = PadColor::Rgb([255, 255, 255]);
    let mut positional: Vec<&str> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--no-stretch" => no_stretch = true,
            "--pad-color" => {
                i += 1;
                let val = args.get(i).ok_or("--pad-color requires a value")?;
                pad_color = parse_pad_color(val)?;
            }
            other if other.starts_with("--") => {
                return Err(format!("Unknown flag: {other}").into());
            }
            other => positional.push(other),
        }
        i += 1;
    }

    if positional.len() < 4 {
        eprintln!(
            "Usage: agno resize [--no-stretch] [--pad-color RRGGBB|RRGGBBAA] <input> <width> <height> <output>"
        );
        return Err("Missing arguments".into());
    }

    let input_path = positional[0];
    let width: u32 = positional[1].parse().map_err(|_| "Invalid width")?;
    let height: u32 = positional[2].parse().map_err(|_| "Invalid height")?;
    let output_path = positional[3];

    let img = load_agno_image_from_file(input_path)?;

    let resized = if no_stretch {
        scale_image_no_stretch(img, width, height, pad_color)?
    } else {
        scale_image(img, width, height)?
    };

    resized.write_to_file(output_path, 100)?;

    if no_stretch {
        let pad_hex = match pad_color {
            PadColor::Rgb([r, g, b]) => format!("{:02x}{:02x}{:02x}", r, g, b),
            PadColor::Rgba([r, g, b, a]) => format!("{:02x}{:02x}{:02x}{:02x}", r, g, b, a),
        };
        println!(
            "Resized {} ({}x{}, no-stretch, pad #{}) -> {}",
            input_path, width, height, pad_hex, output_path
        );
    } else {
        println!(
            "Resized {} ({}x{}) -> {}",
            input_path, width, height, output_path
        );
    }
    Ok(())
}

#[cfg(not(feature = "jpeg"))]
fn cmd_resize(_args: &[String]) -> Result<(), Box<dyn Error>> {
    Err("Resize command requires the 'jpeg' feature.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pad_color_6_digits_returns_rgb() {
        match parse_pad_color("ff8800").unwrap() {
            PadColor::Rgb(b) => assert_eq!(b, [0xFF, 0x88, 0x00]),
            _ => panic!("expected Rgb variant"),
        }
    }

    #[test]
    fn parse_pad_color_8_digits_returns_rgba() {
        match parse_pad_color("ff880080").unwrap() {
            PadColor::Rgba(b) => assert_eq!(b, [0xFF, 0x88, 0x00, 0x80]),
            _ => panic!("expected Rgba variant"),
        }
    }

    #[test]
    fn parse_pad_color_accepts_hash_and_0x_prefix() {
        assert!(matches!(
            parse_pad_color("#000000").unwrap(),
            PadColor::Rgb([0, 0, 0])
        ));
        assert!(matches!(
            parse_pad_color("0xffffffff").unwrap(),
            PadColor::Rgba([0xFF, 0xFF, 0xFF, 0xFF])
        ));
    }

    #[test]
    fn parse_pad_color_rejects_bad_length() {
        assert!(parse_pad_color("abc").is_err());
        assert!(parse_pad_color("abcdefg").is_err());
        assert!(parse_pad_color("abcdefghi").is_err());
    }

    #[test]
    fn parse_pad_color_rejects_non_hex() {
        assert!(parse_pad_color("zzzzzz").is_err());
        assert!(parse_pad_color("zzzzzzzz").is_err());
    }
}
