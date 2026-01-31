use std::env;
use std::error::Error;
use std::fs::File;

use crate::agno_image::load::load_agno_image_from_file;
use crate::exif::{ExifContext, ExifValue};

mod agno_image;
mod canon_decoder;
mod demosaic;
mod exif;
mod sony_decoder;
mod sony_jpeg;
mod tiff;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <command> [args...]", args[0]);
        eprintln!("Commands:");
        eprintln!("  exif <file>                 Print EXIF data for a file");
        eprintln!("  convert <input> <output>    Convert image between formats");
        return Ok(());
    }

    match args[1].as_str() {
        "exif" => cmd_exif(&args[2..]),
        "convert" => cmd_convert(&args[2..]),
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

fn cmd_convert(args: &[String]) -> Result<(), Box<dyn Error>> {
    if args.len() < 2 {
        eprintln!("Usage: agno convert <input> <output>");
        return Err("Missing input or output path".into());
    }

    let input_path = &args[0];
    let output_path = &args[1];

    // Load image (auto-detects format)
    let img = load_agno_image_from_file(input_path)?;
    img.to_jpeg(100, output_path)?;

    println!("Converted {} -> {}", input_path, output_path);
    Ok(())
}
