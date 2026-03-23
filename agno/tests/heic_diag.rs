use agno::codec::heif::parse_heif;
use agno::codec::hevc::bitstream::BitReader;
use agno::codec::hevc::decode_hevc_still;
use agno::codec::hevc::params::{Pps, Sps, Vps};
use std::fs::File;
use std::io::{Seek, SeekFrom};

fn remove_emulation_prevention(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if i + 2 < data.len() && data[i] == 0x00 && data[i + 1] == 0x00 && data[i + 2] == 0x03 {
            out.push(0x00);
            out.push(0x00);
            i += 3;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

fn parse_hvcc_header(hvcc: &[u8]) {
    if hvcc.len() < 23 {
        eprintln!("hvcC too short: {} bytes", hvcc.len());
        return;
    }

    let config_version = hvcc[0];
    let general_profile_idc = hvcc[1] & 0x1F;
    let general_level_idc = hvcc[12];
    let chroma_format = hvcc[13] & 0x03;
    let bit_depth_luma = ((hvcc[14]) & 0x07) + 8;
    let bit_depth_chroma = ((hvcc[15]) & 0x07) + 8;
    let length_size_minus1 = hvcc[21] & 0x03;
    let num_arrays = hvcc[22];

    eprintln!("=== hvcC Header ===");
    eprintln!("  configVersion: {}", config_version);
    eprintln!("  general_profile_idc: {}", general_profile_idc);
    eprintln!("  general_level_idc: {}", general_level_idc);
    eprintln!("  chroma_format: {}", chroma_format);
    eprintln!("  bit_depth_luma: {}", bit_depth_luma);
    eprintln!("  bit_depth_chroma: {}", bit_depth_chroma);
    eprintln!("  NAL length size: {}", length_size_minus1 + 1);
    eprintln!("  num_arrays: {}", num_arrays);

    let mut pos = 23usize;
    for arr in 0..num_arrays as usize {
        if pos + 3 > hvcc.len() {
            break;
        }
        let nal_type = hvcc[pos] & 0x3F;
        let num_nalus = u16::from_be_bytes([hvcc[pos + 1], hvcc[pos + 2]]) as usize;
        pos += 3;
        let type_name = match nal_type {
            32 => "VPS",
            33 => "SPS",
            34 => "PPS",
            _ => "OTHER",
        };
        eprintln!(
            "  Array {}: type={} ({}) count={}",
            arr, nal_type, type_name, num_nalus
        );
        for _ in 0..num_nalus {
            if pos + 2 > hvcc.len() {
                break;
            }
            let nal_len = u16::from_be_bytes([hvcc[pos], hvcc[pos + 1]]) as usize;
            pos += 2;
            if pos + nal_len > hvcc.len() {
                break;
            }
            pos += nal_len;
        }
    }
}

fn parse_and_print_sps_pps(hvcc: &[u8]) {
    if hvcc.len() < 23 {
        return;
    }
    let num_arrays = hvcc[22] as usize;
    let mut pos = 23;
    let mut vps_payloads: Vec<Vec<u8>> = Vec::new();
    let mut sps_payloads: Vec<Vec<u8>> = Vec::new();
    let mut pps_payloads: Vec<Vec<u8>> = Vec::new();

    for _ in 0..num_arrays {
        if pos + 3 > hvcc.len() {
            break;
        }
        let nal_type = hvcc[pos] & 0x3F;
        let num_nalus = u16::from_be_bytes([hvcc[pos + 1], hvcc[pos + 2]]) as usize;
        pos += 3;
        for _ in 0..num_nalus {
            if pos + 2 > hvcc.len() {
                break;
            }
            let nal_len = u16::from_be_bytes([hvcc[pos], hvcc[pos + 1]]) as usize;
            pos += 2;
            if pos + nal_len > hvcc.len() {
                break;
            }
            let nal_data = &hvcc[pos..pos + nal_len];
            pos += nal_len;
            if nal_data.len() < 2 {
                continue;
            }
            let payload = nal_data[2..].to_vec();
            match nal_type {
                32 => vps_payloads.push(payload),
                33 => sps_payloads.push(payload),
                34 => pps_payloads.push(payload),
                _ => {}
            }
        }
    }

    if let Some(vps_data) = vps_payloads.first() {
        let rbsp = remove_emulation_prevention(vps_data);
        let mut r = BitReader::new(&rbsp);
        match Vps::parse(&mut r) {
            Ok(vps) => {
                eprintln!("\n=== VPS ===");
                eprintln!("  max_sub_layers_minus1: {}", vps.max_sub_layers_minus1);
            }
            Err(e) => eprintln!("\n  VPS parse error: {}", e),
        }
    }

    if let Some(sps_data) = sps_payloads.first() {
        let rbsp = remove_emulation_prevention(sps_data);
        let mut r = BitReader::new(&rbsp);
        match Sps::parse(&mut r, 0) {
            Ok(sps) => {
                eprintln!("\n=== SPS ===");
                eprintln!("  pic_width:  {}", sps.pic_width_in_luma_samples);
                eprintln!("  pic_height: {}", sps.pic_height_in_luma_samples);
                eprintln!("  chroma_format_idc: {}", sps._chroma_format_idc);
                eprintln!("  bit_depth_luma: {}", sps.bit_depth_luma);
                eprintln!("  bit_depth_chroma: {}", sps.bit_depth_chroma);
                eprintln!(
                    "  MinCbLog2: {} (MinCb={})",
                    sps.min_cb_log2_size(),
                    sps.min_cb_size()
                );
                eprintln!(
                    "  CtbLog2:   {} (CTB={})",
                    sps.ctb_log2_size(),
                    sps.ctb_size()
                );
                eprintln!("  MinTBLog2: {}", sps.min_tb_log2_size());
                eprintln!("  MaxTBLog2: {}", sps.max_tb_log2_size());
                eprintln!(
                    "  log2_min_luma_coding_block_size_minus3: {}",
                    sps.log2_min_luma_coding_block_size_minus3
                );
                eprintln!(
                    "  log2_diff_max_min_luma_coding_block_size: {}",
                    sps.log2_diff_max_min_luma_coding_block_size
                );
                eprintln!(
                    "  log2_min_luma_transform_block_size_minus2: {}",
                    sps.log2_min_luma_transform_block_size_minus2
                );
                eprintln!(
                    "  log2_diff_max_min_luma_transform_block_size: {}",
                    sps.log2_diff_max_min_luma_transform_block_size
                );
                eprintln!(
                    "  max_transform_hierarchy_depth_intra: {}",
                    sps.max_transform_hierarchy_depth_intra
                );
                eprintln!(
                    "  max_transform_hierarchy_depth_inter: {}",
                    sps._max_transform_hierarchy_depth_inter
                );
                eprintln!(
                    "  scaling_list_enabled_flag: {}",
                    sps.scaling_list_enabled_flag
                );
                eprintln!(
                    "  sample_adaptive_offset_enabled_flag: {}",
                    sps.sample_adaptive_offset_enabled_flag
                );
                eprintln!(
                    "  strong_intra_smoothing_enabled_flag: {}",
                    sps.strong_intra_smoothing_enabled_flag
                );
                eprintln!("  pcm_enabled_flag: {}", sps._pcm_enabled_flag);
                eprintln!(
                    "  num_short_term_ref_pic_sets: {}",
                    sps._num_short_term_ref_pic_sets
                );
                eprintln!(
                    "  long_term_ref_pics_present_flag: {}",
                    sps._long_term_ref_pics_present_flag
                );
                eprintln!(
                    "  sps_temporal_mvp_enabled_flag: {}",
                    sps._sps_temporal_mvp_enabled_flag
                );
                eprintln!(
                    "  vui_parameters_present_flag: {}",
                    sps._vui_parameters_present_flag
                );
                eprintln!(
                    "  CTBs: {}x{} = {}",
                    sps.pic_width_in_ctbs(),
                    sps.pic_height_in_ctbs(),
                    sps.pic_width_in_ctbs() * sps.pic_height_in_ctbs()
                );
            }
            Err(e) => eprintln!("\n  SPS parse error: {}", e),
        }
    }

    if let Some(pps_data) = pps_payloads.first() {
        let rbsp = remove_emulation_prevention(pps_data);
        let mut r = BitReader::new(&rbsp);
        match Pps::parse(&mut r) {
            Ok(pps) => {
                eprintln!("\n=== PPS ===");
                eprintln!(
                    "  init_qp_minus26: {} (slice_qp_base={})",
                    pps.init_qp_minus26,
                    26 + pps.init_qp_minus26
                );
                eprintln!(
                    "  sign_data_hiding_enabled_flag: {}",
                    pps.sign_data_hiding_enabled_flag
                );
                eprintln!(
                    "  cu_qp_delta_enabled_flag: {}",
                    pps._cu_qp_delta_enabled_flag
                );
                eprintln!("  diff_cu_qp_delta_depth: {}", pps._diff_cu_qp_delta_depth);
                eprintln!(
                    "  transform_skip_enabled_flag: {}",
                    pps.transform_skip_enabled_flag
                );
                eprintln!(
                    "  constrained_intra_pred_flag: {}",
                    pps._constrained_intra_pred_flag
                );
                eprintln!("  tiles_enabled_flag: {}", pps._tiles_enabled_flag);
                if pps._tiles_enabled_flag {
                    eprintln!("    num_tile_columns: {}", pps._num_tile_columns_minus1 + 1);
                    eprintln!("    num_tile_rows: {}", pps._num_tile_rows_minus1 + 1);
                    eprintln!("    uniform_spacing_flag: {}", pps._uniform_spacing_flag);
                }
                eprintln!(
                    "  entropy_coding_sync_enabled_flag (WPP): {}",
                    pps._entropy_coding_sync_enabled_flag
                );
                eprintln!(
                    "  cabac_init_present_flag: {}",
                    pps._cabac_init_present_flag
                );
                eprintln!("  pps_cb_qp_offset: {}", pps.pps_cb_qp_offset);
                eprintln!("  pps_cr_qp_offset: {}", pps.pps_cr_qp_offset);
                eprintln!(
                    "  pps_slice_chroma_qp_offsets_present_flag: {}",
                    pps.pps_slice_chroma_qp_offsets_present_flag
                );
                eprintln!(
                    "  deblocking_filter_override_enabled_flag: {}",
                    pps.deblocking_filter_override_enabled_flag
                );
                eprintln!(
                    "  pps_deblocking_filter_disabled_flag: {}",
                    pps.pps_deblocking_filter_disabled_flag
                );
                eprintln!("  weighted_pred_flag: {}", pps._weighted_pred_flag);
                eprintln!("  weighted_bipred_flag: {}", pps._weighted_bipred_flag);
                eprintln!(
                    "  transquant_bypass_enabled_flag: {}",
                    pps._transquant_bypass_enabled_flag
                );
                eprintln!(
                    "  dependent_slice_segments_enabled_flag: {}",
                    pps._dependent_slice_segments_enabled_flag
                );
                eprintln!(
                    "  output_flag_present_flag: {}",
                    pps.output_flag_present_flag
                );
                eprintln!(
                    "  num_extra_slice_header_bits: {}",
                    pps.num_extra_slice_header_bits
                );
                eprintln!(
                    "  lists_modification_present_flag: {}",
                    pps._lists_modification_present_flag
                );
                eprintln!(
                    "  slice_segment_header_extension_present_flag: {}",
                    pps._slice_segment_header_extension_present_flag
                );
            }
            Err(e) => eprintln!("\n  PPS parse error: {}", e),
        }
    }
}

fn analyze_file(label: &str, path: &str) {
    eprintln!("\n============================================================");
    eprintln!("===== {} =====", label);
    eprintln!("File: {}", path);

    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("  SKIP: {}", e);
            return;
        }
    };

    let file_size = file.seek(SeekFrom::End(0)).unwrap();
    file.rewind().unwrap();
    eprintln!(
        "File size: {} bytes ({:.1} MB)",
        file_size,
        file_size as f64 / 1_048_576.0
    );

    let heif = parse_heif(&mut file).unwrap();
    eprintln!("\n=== HEIF Container ===");
    eprintln!(
        "  Grid: {}x{} (cols x rows)",
        heif.grid_cols, heif.grid_rows
    );
    eprintln!("  Output dimensions: {}x{}", heif.width, heif.height);
    eprintln!("  Total tiles: {}", heif.tiles.len());
    eprintln!("  hvcC size: {} bytes", heif.hvcc.len());

    let tile_sizes: Vec<usize> = heif.tiles.iter().map(|t| t.len()).collect();
    let min_tile = tile_sizes.iter().min().unwrap_or(&0);
    let max_tile = tile_sizes.iter().max().unwrap_or(&0);
    let avg_tile: f64 = tile_sizes.iter().sum::<usize>() as f64 / tile_sizes.len().max(1) as f64;
    eprintln!(
        "  Tile bitstream sizes: min={} max={} avg={:.0}",
        min_tile, max_tile, avg_tile
    );

    for (i, size) in tile_sizes.iter().enumerate().take(60) {
        eprintln!("    tile[{}]: {} bytes", i, size);
    }
    if tile_sizes.len() > 60 {
        eprintln!("    ... ({} more tiles)", tile_sizes.len() - 60);
    }

    parse_hvcc_header(&heif.hvcc);
    parse_and_print_sps_pps(&heif.hvcc);

    // Examine first tile's NAL units
    if let Some(tile0) = heif.tiles.first() {
        let nal_length_size = ((heif.hvcc[21] & 0x03) + 1) as usize;
        eprintln!(
            "\n=== First Tile NAL Units (length_size={}) ===",
            nal_length_size
        );
        let mut pos = 0;
        let mut nal_idx = 0;
        while pos + nal_length_size <= tile0.len() {
            let nal_len = match nal_length_size {
                1 => tile0[pos] as usize,
                2 => u16::from_be_bytes([tile0[pos], tile0[pos + 1]]) as usize,
                4 => {
                    u32::from_be_bytes([tile0[pos], tile0[pos + 1], tile0[pos + 2], tile0[pos + 3]])
                        as usize
                }
                _ => break,
            };
            pos += nal_length_size;
            if pos + nal_len > tile0.len() {
                break;
            }
            if nal_len >= 2 {
                let nal_type = (tile0[pos] >> 1) & 0x3F;
                let nal_type_name = match nal_type {
                    0 => "TRAIL_N",
                    1 => "TRAIL_R",
                    19 => "IDR_W_RADL",
                    20 => "IDR_N_LP",
                    21 => "CRA_NUT",
                    32 => "VPS",
                    33 => "SPS",
                    34 => "PPS",
                    _ => "OTHER",
                };
                eprintln!(
                    "  NAL[{}]: type={} ({}) size={}",
                    nal_idx, nal_type, nal_type_name, nal_len
                );
            }
            pos += nal_len;
            nal_idx += 1;
        }
    }
}

#[test]
fn diagnose_heic_structure() {
    let test_file = "/Users/ethan/repos/weblens/_build/fs/core/data/users/admin/photos-export/282980C6-12C5-4C53-8267-C054B9B40BEE.heic";
    let ref_file = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/sideways2.heic");

    analyze_file("TARGET IMAGE", test_file);
    analyze_file("REFERENCE (sideways2.heic)", ref_file);
}

#[test]
fn decode_target_tiles() {
    let test_file = "/Users/ethan/repos/weblens/_build/fs/core/data/users/admin/photos-export/282980C6-12C5-4C53-8267-C054B9B40BEE.heic";

    let mut file = File::open(test_file).unwrap();
    let heif = parse_heif(&mut file).unwrap();

    eprintln!(
        "Grid: {}x{}, {} tiles total",
        heif.grid_cols,
        heif.grid_rows,
        heif.tiles.len()
    );
    eprintln!("Output: {}x{}", heif.width, heif.height);

    let mut success_count = 0;
    let mut error_count = 0;
    let mut error_tiles: Vec<(usize, usize, usize, String)> = Vec::new();

    for (i, tile_data) in heif.tiles.iter().enumerate() {
        let row = i / heif.grid_cols as usize;
        let col = i % heif.grid_cols as usize;

        match decode_hevc_still(&heif.hvcc, tile_data) {
            Ok(pic) => {
                let rgb = pic.to_rgb8();
                let total_pixels = (pic.width * pic.height) as usize;
                let mut green_count = 0u64;
                let mut zero_count = 0u64;
                let mut saturated_count = 0u64;

                for p in 0..total_pixels {
                    let r = rgb[p * 3] as u16;
                    let g = rgb[p * 3 + 1] as u16;
                    let b = rgb[p * 3 + 2] as u16;
                    if g > r + 30 && g > b + 30 {
                        green_count += 1;
                    }
                    if r == 0 && g == 0 && b == 0 {
                        zero_count += 1;
                    }
                    if r == 255 || g == 255 || b == 255 {
                        saturated_count += 1;
                    }
                }

                let green_pct = green_count as f64 / total_pixels as f64 * 100.0;
                let zero_pct = zero_count as f64 / total_pixels as f64 * 100.0;
                let sat_pct = saturated_count as f64 / total_pixels as f64 * 100.0;

                let sum: u64 = rgb.iter().map(|&v| v as u64).sum();
                let avg = sum as f64 / rgb.len() as f64;

                let status = if green_pct > 20.0 {
                    "SUSPECT_GREEN"
                } else if zero_pct > 50.0 {
                    "SUSPECT_BLACK"
                } else if sat_pct > 50.0 {
                    "SUSPECT_SATURATED"
                } else {
                    "OK"
                };

                eprintln!(
                    "Tile {:>3} [{:>2},{:>2}] {}x{}: {} (green={:.1}% zero={:.1}% sat={:.1}% avg={:.1})",
                    i, row, col, pic.width, pic.height, status, green_pct, zero_pct, sat_pct, avg
                );
                success_count += 1;
            }
            Err(e) => {
                let err_msg = format!("{}", e);
                eprintln!(
                    "Tile {:>3} [{:>2},{:>2}]: DECODE_ERROR: {}",
                    i, row, col, err_msg
                );
                error_tiles.push((i, row, col, err_msg));
                error_count += 1;
            }
        }
    }

    eprintln!("\n=== Summary ===");
    eprintln!("Success: {}", success_count);
    eprintln!("Errors:  {}", error_count);
    if !error_tiles.is_empty() {
        eprintln!("\nFailed tiles:");
        for (i, row, col, err) in &error_tiles {
            eprintln!("  Tile {} [{},{}]: {}", i, row, col, err);
        }
    }
}

fn calculate_psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "Image sizes must match");
    let mse: f64 = a
        .iter()
        .zip(b.iter())
        .map(|(&x, &y)| {
            let diff = x as f64 - y as f64;
            diff * diff
        })
        .sum::<f64>()
        / a.len() as f64;

    if mse == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

#[test]
fn compare_tiles_to_reference() {
    let test_file = "/Users/ethan/repos/weblens/_build/fs/core/data/users/admin/photos-export/282980C6-12C5-4C53-8267-C054B9B40BEE.heic";
    let ref_rgb_path = "/tmp/target-reference-full.rgb";

    if !std::path::Path::new(ref_rgb_path).exists() {
        eprintln!(
            "SKIP: reference RGB not found at {}. Generate with:",
            ref_rgb_path
        );
        eprintln!(
            "  heif-convert file.heic /tmp/ref.png && ffmpeg -i /tmp/ref.png -pix_fmt rgb24 -f rawvideo /tmp/target-reference-full.rgb"
        );
        return;
    }

    let reference = std::fs::read(ref_rgb_path).unwrap();
    let mut file = File::open(test_file).unwrap();
    let heif = parse_heif(&mut file).unwrap();

    let out_w = heif.width as usize;
    let out_h = heif.height as usize;
    let cols = heif.grid_cols as usize;

    assert_eq!(
        reference.len(),
        out_w * out_h * 3,
        "Reference size {} != expected {} for {}x{}",
        reference.len(),
        out_w * out_h * 3,
        out_w,
        out_h
    );

    // Decode all tiles and stitch
    let mut decoded_rgb = vec![0u8; out_w * out_h * 3];

    for (i, tile_data) in heif.tiles.iter().enumerate() {
        let tile_row = i / cols;
        let tile_col = i % cols;

        let pic = decode_hevc_still(&heif.hvcc, tile_data).unwrap();
        let tile_rgb = pic.to_rgb8();
        let tw = pic.width as usize;
        let th = pic.height as usize;
        let ox = tile_col * tw;
        let oy = tile_row * th;

        for ty in 0..th {
            let dy = oy + ty;
            if dy >= out_h {
                break;
            }
            for tx in 0..tw {
                let dx = ox + tx;
                if dx >= out_w {
                    break;
                }
                let src = (ty * tw + tx) * 3;
                let dst = (dy * out_w + dx) * 3;
                decoded_rgb[dst..dst + 3].copy_from_slice(&tile_rgb[src..src + 3]);
            }
        }
    }

    // Full image PSNR
    let full_psnr = calculate_psnr(&decoded_rgb, &reference);
    eprintln!("Full image RGB PSNR: {:.2} dB", full_psnr);

    // Per-tile PSNR
    let mut tile_psnrs: Vec<(usize, usize, usize, f64)> = Vec::new();
    let tile_w = 512usize;
    let tile_h = 512usize;

    for (i, _tile_data) in heif.tiles.iter().enumerate() {
        let tile_row = i / cols;
        let tile_col = i % cols;
        let ox = tile_col * tile_w;
        let oy = tile_row * tile_h;

        let mut tile_dec = Vec::new();
        let mut tile_ref = Vec::new();

        for ty in 0..tile_h {
            let dy = oy + ty;
            if dy >= out_h {
                break;
            }
            for tx in 0..tile_w {
                let dx = ox + tx;
                if dx >= out_w {
                    break;
                }
                let idx = (dy * out_w + dx) * 3;
                tile_dec.extend_from_slice(&decoded_rgb[idx..idx + 3]);
                tile_ref.extend_from_slice(&reference[idx..idx + 3]);
            }
        }

        let psnr = calculate_psnr(&tile_dec, &tile_ref);
        tile_psnrs.push((i, tile_row, tile_col, psnr));
    }

    // Sort by PSNR ascending (worst first)
    tile_psnrs.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap());

    eprintln!("\n=== Per-tile PSNR (worst first) ===");
    for (i, row, col, psnr) in &tile_psnrs {
        let status = if *psnr < 25.0 {
            "BAD"
        } else if *psnr < 30.0 {
            "POOR"
        } else if *psnr < 35.0 {
            "FAIR"
        } else {
            "GOOD"
        };
        eprintln!(
            "Tile {:>3} [{:>2},{:>2}] PSNR={:>6.2} dB  {}",
            i, row, col, psnr, status
        );
    }

    // Dump decoded to file for visual comparison
    std::fs::write("/tmp/target-decoded.rgb", &decoded_rgb).unwrap();
    eprintln!("\nDecoded RGB dumped to /tmp/target-decoded.rgb");
    eprintln!(
        "Convert: ffmpeg -f rawvideo -pix_fmt rgb24 -s {}x{} -i /tmp/target-decoded.rgb /tmp/target-decoded.png",
        out_w, out_h
    );

    // Count tiles below threshold
    let bad_count = tile_psnrs.iter().filter(|t| t.3 < 25.0).count();
    let poor_count = tile_psnrs
        .iter()
        .filter(|t| t.3 >= 25.0 && t.3 < 30.0)
        .count();
    eprintln!(
        "\nBAD (<25dB): {}, POOR (25-30dB): {}, OK (>30dB): {}",
        bad_count,
        poor_count,
        tile_psnrs.len() - bad_count - poor_count
    );
}

#[test]
fn compare_sideways2_tiles_to_reference() {
    let test_file = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/sideways2.heic");
    let ref_rgb_path = "/tmp/sideways2-ref.rgb";

    if !std::path::Path::new(ref_rgb_path).exists() {
        eprintln!("SKIP: reference RGB not found at {}", ref_rgb_path);
        return;
    }

    let reference = std::fs::read(ref_rgb_path).unwrap();
    let mut file = File::open(test_file).unwrap();
    let heif = parse_heif(&mut file).unwrap();

    let out_w = heif.width as usize;
    let out_h = heif.height as usize;
    let cols = heif.grid_cols as usize;

    // Decode and stitch
    let mut decoded_rgb = vec![0u8; out_w * out_h * 3];
    for (i, tile_data) in heif.tiles.iter().enumerate() {
        let tile_row = i / cols;
        let tile_col = i % cols;
        let pic = decode_hevc_still(&heif.hvcc, tile_data).unwrap();
        let tile_rgb = pic.to_rgb8();
        let tw = pic.width as usize;
        let th = pic.height as usize;
        let ox = tile_col * tw;
        let oy = tile_row * th;
        for ty in 0..th {
            let dy = oy + ty;
            if dy >= out_h {
                break;
            }
            for tx in 0..tw {
                let dx = ox + tx;
                if dx >= out_w {
                    break;
                }
                let src = (ty * tw + tx) * 3;
                let dst = (dy * out_w + dx) * 3;
                decoded_rgb[dst..dst + 3].copy_from_slice(&tile_rgb[src..src + 3]);
            }
        }
    }

    let full_psnr = calculate_psnr(&decoded_rgb, &reference);
    eprintln!("sideways2.heic Full image RGB PSNR: {:.2} dB", full_psnr);

    let tile_w = 512usize;
    let tile_h = 512usize;
    let mut tile_psnrs: Vec<(usize, usize, usize, f64)> = Vec::new();

    for i in 0..heif.tiles.len() {
        let tile_row = i / cols;
        let tile_col = i % cols;
        let ox = tile_col * tile_w;
        let oy = tile_row * tile_h;
        let mut tile_dec = Vec::new();
        let mut tile_ref = Vec::new();
        for ty in 0..tile_h {
            let dy = oy + ty;
            if dy >= out_h {
                break;
            }
            for tx in 0..tile_w {
                let dx = ox + tx;
                if dx >= out_w {
                    break;
                }
                let idx = (dy * out_w + dx) * 3;
                tile_dec.extend_from_slice(&decoded_rgb[idx..idx + 3]);
                tile_ref.extend_from_slice(&reference[idx..idx + 3]);
            }
        }
        let psnr = calculate_psnr(&tile_dec, &tile_ref);
        tile_psnrs.push((i, tile_row, tile_col, psnr));
    }

    tile_psnrs.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap());

    eprintln!("\n=== sideways2.heic Per-tile PSNR (worst first) ===");
    for (i, row, col, psnr) in &tile_psnrs {
        let status = if *psnr < 25.0 {
            "BAD"
        } else if *psnr < 30.0 {
            "POOR"
        } else if *psnr < 35.0 {
            "FAIR"
        } else {
            "GOOD"
        };
        eprintln!(
            "Tile {:>3} [{:>2},{:>2}] PSNR={:>6.2} dB  {}",
            i, row, col, psnr, status
        );
    }

    let bad_count = tile_psnrs.iter().filter(|t| t.3 < 25.0).count();
    let poor_count = tile_psnrs
        .iter()
        .filter(|t| t.3 >= 25.0 && t.3 < 30.0)
        .count();
    eprintln!(
        "\nBAD (<25dB): {}, POOR (25-30dB): {}, OK (>30dB): {}",
        bad_count,
        poor_count,
        tile_psnrs.len() - bad_count - poor_count
    );
}
