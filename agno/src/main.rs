use std::env;
use std::error::Error;
use std::fs::File;

use agno::agno_image::load::load_agno_image_from_file;
use agno::agno_image::transform::scale_image;
use agno::exif::{ExifContext, ExifValue};
use agno::logging::{LogConfig, init};

fn main() -> Result<(), Box<dyn Error>> {
    init(LogConfig::cli());
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <command> [args...]", args[0]);
        eprintln!("Commands:");
        eprintln!("  exif <file>                              Print EXIF data for a file");
        eprintln!("  convert <input> <output>                 Convert image between formats");
        eprintln!(
            "  resize <input> <width> <height> <output> Resize image to specified dimensions"
        );
        return Ok(());
    }

    match args[1].as_str() {
        "exif" => cmd_exif(&args[2..]),
        "convert" => cmd_convert(&args[2..]),
        "resize" => cmd_resize(&args[2..]),
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

#[cfg(feature = "jpeg")]
fn cmd_convert(args: &[String]) -> Result<(), Box<dyn Error>> {
    if args.len() < 2 {
        eprintln!("Usage: agno convert [options] <input> <output>");
        eprintln!("Options:");
        eprintln!("  --page SPEC    Page selection for PDF (default: 1)");
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

    let is_pdf = input_path.ends_with(".pdf")
        || input_path.ends_with(".PDF")
        || std::fs::File::open(input_path)
            .ok()
            .and_then(|mut f| {
                use std::io::Read;
                let mut header = [0u8; 5];
                f.read_exact(&mut header).ok()?;
                Some(header)
            })
            .map(|h| &h == b"%PDF-")
            .unwrap_or(false);

    if is_pdf && page_spec.is_some() {
        convert_pdf(input_path, output_path, page_spec.as_deref().unwrap())?;
    } else {
        let img = load_agno_image_from_file(input_path)?;
        img.write_to_file(output_path, 100)?;
    }

    println!("Converted {} -> {}", input_path, output_path);
    Ok(())
}

#[cfg(all(feature = "jpeg", feature = "pdf"))]
fn convert_pdf(input: &str, output: &str, page_spec: &str) -> Result<(), Box<dyn Error>> {
    use agno::agno_image::AgnoImage;
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
        let mut pages: Vec<AgnoImage> = Vec::new();
        for &p in &page_indices {
            let img = agno::agno_image::load::load_pdf_page_from_bytes(
                &data,
                p,
                None,
                ExifContext::default(),
            )?;
            pages.push(img);
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
            for row in 0..page_img.height {
                let src_start = (row * page_img.width * 3) as usize;
                let src_end = src_start + (page_img.width * 3) as usize;
                let dst_start = ((y_offset + row) * max_w * 3) as usize;
                let copy_len = (page_img.width * 3) as usize;
                combined[dst_start..dst_start + copy_len].copy_from_slice(&src[src_start..src_end]);
            }
            y_offset += page_img.height;
        }

        let out = AgnoImage::new(combined, max_w, total_h, ExifContext::default());
        out.write_to_file(output, 100)?;
        println!("Rendered {} pages ({max_w}x{total_h})", page_indices.len());
    }
    Ok(())
}

#[cfg(all(feature = "jpeg", not(feature = "pdf")))]
fn convert_pdf(_input: &str, _output: &str, _page_spec: &str) -> Result<(), Box<dyn Error>> {
    Err("PDF support requires the 'pdf' feature.".into())
}

#[cfg(not(feature = "jpeg"))]
fn cmd_convert(_args: &[String]) -> Result<(), Box<dyn Error>> {
    Err("Convert command requires the 'jpeg' feature.".into())
}

#[cfg(feature = "jpeg")]
fn cmd_resize(args: &[String]) -> Result<(), Box<dyn Error>> {
    if args.len() < 4 {
        eprintln!("Usage: agno resize <input> <width> <height> <output>");
        return Err("Missing arguments".into());
    }

    let input_path = &args[0];
    let width: u32 = args[1].parse().map_err(|_| "Invalid width")?;
    let height: u32 = args[2].parse().map_err(|_| "Invalid height")?;
    let output_path = &args[3];

    // Load image (auto-detects format)
    let img = load_agno_image_from_file(input_path)?;

    // Scale using GPU-accelerated resize (with CPU fallback)
    let resized = scale_image(img, width, height)?;

    // Write output as JPEG
    resized.to_jpeg_file(100, output_path)?;

    println!(
        "Resized {} ({}x{}) -> {}",
        input_path, width, height, output_path
    );
    Ok(())
}

#[cfg(not(feature = "jpeg"))]
fn cmd_resize(_args: &[String]) -> Result<(), Box<dyn Error>> {
    Err("Resize command requires the 'jpeg' feature.".into())
}
