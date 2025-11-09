use crate::agno_image::load::load_agno_image_from_file;

mod agno_image;
mod demosaic;
mod exif;

mod sony_decoder;
mod sony_jpeg;
mod tiff;

// extern crate log;
// extern crate rayon;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // start_stopwatch();
    //
    // env_logger::builder()
    //     .filter_level(LevelFilter::Info)
    //     .format_target(false)
    //     .format_level(true)
    //     .format_indent(Some(4))
    //     .format_file(true)
    //     .format_line_number(true)
    //     .init();
    //
    // let args: Vec<String> = env::args().collect();
    // if args.len() < 2 {
    //     eprintln!("Usage: {} [cmd] [args...]", args[0]);
    //     return Err("No file path provided".into());
    // }
    //
    // let command = &args[1];
    //
    // if let Err(err) = match command.as_str() {
    //     "exif" => print_exif(args),
    //     "convert" => convert_file(args),
    //     _ => Err("Invalid command".into()),
    // } {
    //     eprintln!("Error: {}", err);
    //     return Err(err);
    // }
    //
    // Ok(())

    load_agno_image_from_file("")?;
    Ok(())
}

// fn print_exif(args: Vec<String>) -> Result<(), Box<dyn Error>> {
//     if args.len() < 3 {
//         eprintln!("Usage: {} exif [file_path]", args[0]);
//         return Err("No file path provided".into());
//     }
//
//     let path = &args[2];
//     let mut file = File::open(path)?;
//
//     let ctx = ExifContext::from_reader_auto(&mut file).expect("no exif found or parse failed");
//
//     let _thing = ctx.get_tag_value(PAGE_NUMBER);
//     let _thing = ctx.get_tag_value(IMAGE_WIDTH);
//     let _thing = ctx.get_tag_value(WB_RGGBLEVELS);
//
//     // if let Err(res) = ctx.print_all_data() {
//     //     return Err(res.into());
//     // }
//
//     Ok(())
// }

// fn parse_exif_wb<R: Read + Seek>(ctx: &mut ExifContext) -> Option<[f32; 3]> {
//     // Prefer DNG AsShotNeutral (3 components). Compute gains so that G is 1.
//     if let Some(val) = ctx.get_tag_value(AS_SHOT_NEUTRAL) {
//         let mut comps: Vec<f32> = Vec::new();
//         match val {
//             ExifValue::Short(v) => comps = v.into_iter().map(|x| x as f32).collect(),
//             ExifValue::Long(v) => comps = v.into_iter().map(|x| x as f32).collect(),
//             ExifValue::Rational(v) => {
//                 comps = v
//                     .into_iter()
//                     .map(|(n, d)| if d == 0 { 0.0 } else { (n as f32) / (d as f32) })
//                     .collect()
//             }
//             ExifValue::SRational(v) => {
//                 comps = v
//                     .into_iter()
//                     .map(|(n, d)| if d == 0 { 0.0 } else { (n as f32) / (d as f32) })
//                     .collect()
//             }
//             _ => {}
//         }
//         if comps.len() >= 3 {
//             let r_n = comps[0].max(1e-6);
//             let g_n = comps[1].max(1e-6);
//             let b_n = comps[2].max(1e-6);
//             let mut r = (g_n / r_n).clamp(0.2, 5.0);
//             let g = 1.0f32;
//             let mut b = (g_n / b_n).clamp(0.2, 5.0);
//             if !r.is_finite() {
//                 r = 1.0;
//             }
//             if !b.is_finite() {
//                 b = 1.0;
//             }
//             return Some([r, g, b]);
//         }
//     }
//
//     // Fallback: Sony maker note WB_RGGBLevels (R, G1, B, G2)
//     if let Ok(Some(val)) = ctx.get_tag_value(WB_RGGBLEVELS.tag) {
//         if let ExifValue::Short(v) = val {
//             if v.len() >= 4 {
//                 let r = v[0] as f32;
//                 let g1 = v[1] as f32;
//                 let b = v[2] as f32;
//                 let g2 = v[3] as f32;
//                 let g_avg = ((g1 + g2) / 2.0).max(1e-6);
//                 let mut r_gain = (r / g_avg).clamp(0.2, 5.0);
//                 let mut b_gain = (b / g_avg).clamp(0.2, 5.0);
//                 if !r_gain.is_finite() {
//                     r_gain = 1.0;
//                 }
//                 if !b_gain.is_finite() {
//                     b_gain = 1.0;
//                 }
//                 return Some([r_gain, 1.0, b_gain]);
//             }
//         }
//     }
//
//     None
// }

// fn convert_file(args: Vec<String>) -> Result<(), Box<dyn Error>> {
//     if args.len() < 4 {
//         eprintln!("Usage: {} convert [in_path] [out_path]", args[0]);
//         return Err("Missing input and output paths".into());
//     }
//
//     let in_path = &args[2];
//     let out_path = &args[3];
//     let mut file = File::open(in_path)?;
//
//     // Read EXIF for WB gains if present
//     // let mut exif_ctx =
//     //     ExifContext::from_reader_auto(&mut file).expect("no exif found or parse failed");
//     // let wb_from_exif = parse_exif_wb(&mut exif_ctx);
//     // if let Some([wr, wg, wb]) = wb_from_exif {
//     //     info!("EXIF WB gains: R={:.4} G={:.4} B={:.4}", wr, wg, wb);
//     // } else {
//     //     info!("No EXIF WB found; will use gray-world estimate");
//     // }
//
//     // Detect ARW variant and raw data strips via TIFF
//     let det = detect_sony_raw(&mut file).expect("TIFF/ARW detect failed");
//
//     let mut dims = Dimensions {
//         raw_width: det.raw.width as usize,
//         raw_height: det.raw.height as usize,
//         output_width: det.raw.width as usize,
//         output_height: det.raw.height as usize,
//     };
//
//     lap!("Detected RAW");
//
//     // Read strips into memory once. Most ARW are single-strip; this works for multi-strip too.
//     let buf = sony_decoder::read_concatenated_strips(
//         &mut file,
//         &det.raw.strip_offsets,
//         &det.raw.strip_byte_counts,
//     )
//     .expect("read strips failed");
//     let mut cursor = Cursor::new(buf);
//
//     // Auto-select decoder based on detection
//     let mut decoded = match det.variant {
//         SonyVariant::Arw2Compressed => {
//             // ARW2: compressed row length equals pixel width; decoder expects row_len == active_width
//             match sony_decoder::sony_arw2_load_raw(&mut cursor, dims) {
//                 Ok(result) => result,
//                 Err(e) => return Err(e.into()),
//             }
//         }
//         SonyVariant::ArwLjpeg => {
//             // Legacy ARW: LibRaw adds 8 rows (raw_height += 8)
//             dims.raw_height = dims.raw_height + 8;
//             // zero_after_ff is usually true for JPEG-like streams
//             let zero_after_ff = true;
//             // Pass DNG version if present to match ljpeg_diff behavior; for native ARW, None is fine
//             let dng_version = det.raw.dng_version;
//             match sony_decoder::sony_arw_load_raw_from_stream(
//                 &mut cursor,
//                 dims,
//                 zero_after_ff,
//                 dng_version,
//             ) {
//                 Ok(result) => result,
//                 Err(e) => return Err(e.into()),
//             }
//         }
//         SonyVariant::Uncompressed14 => {
//             // Simple 14-bit uncompressed packed in 16-bit little-endian words
//             match sony_decoder::sony_uncompressed14_load_raw(&mut cursor, dims) {
//                 Ok(result) => result,
//                 Err(e) => return Err(e.into()),
//             }
//         }
//         SonyVariant::Unknown => return Err("Unknown Sony ARW variant; not handled".into()),
//     };
//
//     // Write a JPEG (choose your CFA pattern and black level)
//     let pattern = BayerPattern::RGGB; // Adjust per camera/margins
//     let black_level = if det.variant != SonyVariant::Arw2Compressed {
//         512
//     } else {
//         0
//     }; // heuristic
//
//     file.seek(SeekFrom::Start(0))?;
//     let mut ctx = ExifContext::from_reader_auto(&mut file)?;
//     ctx.get_tag_value(BLACK_LEVEL)?;
//
//     lap!("Decoded RAW");
//
//     let gamma = 2.2;
//     let quality = 90u8;
//
//     let rgb = demosaic_bilinear_to_rgb8(
//         &mut decoded.pixels,
//         dims,
//         pattern,
//         black_level,
//         decoded.white_level,
//         [2.766637, 1.0, 1.427233], // wb_gains (EXIF if present; None falls back to gray-world)
//         gamma,
//     );
//
//     lap!("Demosaiced to RGB8");
//
//     let mut out_file = match File::create(out_path) {
//         Ok(f) => f,
//         Err(_) => {
//             warn!("Failed to create out_file at {}", out_path);
//             return Err("Failed to create output file".into());
//         }
//     };
//
//     let rot = ctx.get_tag_value(ORIENTATION.tag).unwrap_or(None);
//     let img = match rot {
//         Some(ExifValue::Short(v)) if v.len() >= 1 => {
//             let exif_orient = v[0] as u8;
//             match exif_orient {
//                 1 | 2 | 3 | 4 | 5 | 7 => &rgb, // No rotation
//                 6 => {
//                     info!("Applying 90-degree rotation from EXIF");
//                     // 90-degree rotation
//                     (dims.output_width, dims.output_height) =
//                         (dims.output_height, dims.output_width);
//
//                     &rgb
//                 }
//                 8 => {
//                     info!("Applying 270-degree rotation from EXIF");
//                     let img =
//                         RgbImage::from_raw(dims.output_width as u32, dims.raw_height as u32, rgb)
//                             .ok_or(DecodeError::CorruptData(
//                             "Failed to create image from RGB data",
//                         ))?;
//
//                     let rotated_img = imageops::rotate270(&img);
//
//                     // 270-degree rotation
//                     (dims.output_width, dims.output_height) =
//                         (dims.output_height, dims.output_width);
//
//                     &rotated_img.as_raw().to_vec()
//                 }
//                 _ => &rgb,
//             }
//         }
//         _ => &rgb,
//     };
//
//     lap!("Applied EXIF rotation");
//
//     info!(
//         "Writing output {}x{} WebP with quality {}",
//         dims.output_width, dims.output_height, quality
//     );
//
//     write_webp_from_rgb8_writer(
//         &mut out_file,
//         &img,
//         dims.output_width as u32,
//         dims.output_height as u32,
//         quality,
//     )
//     .expect("webp write failed");
//
//     lap!("Wrote Output");
//
//     stop_stopwatch();
//
//     Ok(())
// }
