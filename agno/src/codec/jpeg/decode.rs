//! Baseline JPEG (SOF0) decoder.
//!
//! Supports 8-bit YCbCr with 4:4:4, 4:2:2, and 4:2:0 chroma subsampling.
//! Handles restart markers (DRI) and arbitrary quantization / Huffman tables.

use std::error::Error;

use super::dct::{dequantize_block, idct8x8};
use super::huffman::{build_huffman_decode_table, HuffmanDecodeTable, JpegBitReader};
use super::tables::DEZIGZAG;

// ---- JPEG marker constants ----

const MARKER_SOI: u8 = 0xD8;
const MARKER_EOI: u8 = 0xD9;
const MARKER_SOF0: u8 = 0xC0;
const MARKER_DHT: u8 = 0xC4;
const MARKER_DQT: u8 = 0xDB;
const MARKER_SOS: u8 = 0xDA;
const MARKER_DRI: u8 = 0xDD;

// ---- Data structures ----

struct FrameHeader {
    width: u16,
    height: u16,
    components: Vec<Component>,
}

#[derive(Clone)]
struct Component {
    id: u8,
    h_sampling: u8,
    v_sampling: u8,
    quant_table_id: u8,
}

struct ScanComponentSelector {
    component_index: usize,
    dc_table_id: u8,
    ac_table_id: u8,
}

struct JpegContext {
    quant_tables: [[u16; 64]; 4],
    dc_huff_tables: [Option<HuffmanDecodeTable>; 4],
    ac_huff_tables: [Option<HuffmanDecodeTable>; 4],
    frame: Option<FrameHeader>,
    restart_interval: u16,
}

impl JpegContext {
    fn new() -> Self {
        Self {
            quant_tables: [[0u16; 64]; 4],
            dc_huff_tables: [const { None }; 4],
            ac_huff_tables: [const { None }; 4],
            frame: None,
            restart_interval: 0,
        }
    }
}

// ---- Public API ----

/// Decode baseline JPEG data into RGB pixels.
///
/// Returns `(rgb_data, width, height)` where `rgb_data` is packed R,G,B bytes
/// in row-major order (length = width * height * 3).
pub fn decode_jpeg(data: &[u8]) -> Result<(Vec<u8>, u32, u32), Box<dyn Error>> {
    if data.len() < 2 || data[0] != 0xFF || data[1] != MARKER_SOI {
        return Err("Not a JPEG file (missing SOI marker)".into());
    }

    let mut ctx = JpegContext::new();
    let mut pos = 2;

    // Scan markers until we hit SOS
    loop {
        let (marker, marker_pos) = next_marker(data, pos)?;
        pos = marker_pos;

        match marker {
            MARKER_DQT => pos = parse_dqt(data, pos, &mut ctx)?,
            MARKER_SOF0 => pos = parse_sof0(data, pos, &mut ctx)?,
            MARKER_DHT => pos = parse_dht(data, pos, &mut ctx)?,
            MARKER_DRI => pos = parse_dri(data, pos, &mut ctx)?,
            MARKER_SOS => {
                let (selectors, entropy_start) = parse_sos(data, pos, &ctx)?;
                let frame = ctx
                    .frame
                    .as_ref()
                    .ok_or("SOS before SOF0")?;

                let rgb = decode_scan(
                    data,
                    entropy_start,
                    frame,
                    &selectors,
                    &ctx,
                )?;

                return Ok((rgb, frame.width as u32, frame.height as u32));
            }
            MARKER_EOI => return Err("Unexpected EOI before SOS".into()),
            // Skip SOF markers we don't support
            m if (0xC1..=0xCF).contains(&m) && m != MARKER_DHT => {
                return Err(format!(
                    "Unsupported JPEG frame type: SOF{} (0xFF{:02X})",
                    m - 0xC0,
                    m
                )
                .into());
            }
            // Skip APPn, COM, and other markers
            _ => {
                let seg_len = read_u16_be(data, pos)? as usize;
                pos += seg_len;
            }
        }
    }
}

// ---- Marker navigation ----

fn next_marker(data: &[u8], mut pos: usize) -> Result<(u8, usize), Box<dyn Error>> {
    // Find next 0xFF byte
    while pos < data.len() {
        if data[pos] == 0xFF {
            pos += 1;
            // Skip padding 0xFF bytes
            while pos < data.len() && data[pos] == 0xFF {
                pos += 1;
            }
            if pos < data.len() && data[pos] != 0x00 {
                let marker = data[pos];
                pos += 1;
                return Ok((marker, pos));
            }
        } else {
            pos += 1;
        }
    }
    Err("Unexpected end of JPEG data while scanning for marker".into())
}

fn read_u16_be(data: &[u8], pos: usize) -> Result<u16, Box<dyn Error>> {
    if pos + 1 >= data.len() {
        return Err("Unexpected end of data reading u16".into());
    }
    Ok(((data[pos] as u16) << 8) | data[pos + 1] as u16)
}

// ---- Marker parsers ----

fn parse_dqt(data: &[u8], pos: usize, ctx: &mut JpegContext) -> Result<usize, Box<dyn Error>> {
    let seg_len = read_u16_be(data, pos)? as usize;
    let end = pos + seg_len;
    let mut p = pos + 2;

    while p < end {
        let pq_tq = data[p];
        let precision = pq_tq >> 4; // 0 = 8-bit, 1 = 16-bit
        let table_id = (pq_tq & 0x0F) as usize;
        p += 1;

        if table_id > 3 {
            return Err(format!("Invalid quantization table ID: {}", table_id).into());
        }

        // DQT values in the file are stored in zigzag order.
        // We store them in natural (row-major) order for use with dequantize_block.
        let mut qt_zigzag = [0u16; 64];
        if precision == 0 {
            for i in 0..64 {
                qt_zigzag[i] = data[p + i] as u16;
            }
            p += 64;
        } else {
            for i in 0..64 {
                qt_zigzag[i] = read_u16_be(data, p + i * 2)?;
            }
            p += 128;
        }

        // Convert from zigzag order to natural order
        let mut qt_natural = [0u16; 64];
        for zigzag_pos in 0..64 {
            qt_natural[DEZIGZAG[zigzag_pos]] = qt_zigzag[zigzag_pos];
        }
        ctx.quant_tables[table_id] = qt_natural;
    }

    Ok(end)
}

fn parse_sof0(data: &[u8], pos: usize, ctx: &mut JpegContext) -> Result<usize, Box<dyn Error>> {
    let seg_len = read_u16_be(data, pos)? as usize;
    let end = pos + seg_len;
    let mut p = pos + 2;

    let precision = data[p];
    p += 1;
    if precision != 8 {
        return Err(format!("Unsupported sample precision: {} (only 8-bit supported)", precision).into());
    }

    let height = read_u16_be(data, p)?;
    p += 2;
    let width = read_u16_be(data, p)?;
    p += 2;
    let num_components = data[p] as usize;
    p += 1;

    if num_components == 0 || num_components > 4 {
        return Err(format!("Unsupported number of components: {}", num_components).into());
    }

    let mut components = Vec::with_capacity(num_components);
    for _ in 0..num_components {
        let id = data[p];
        let sampling = data[p + 1];
        let h_sampling = sampling >> 4;
        let v_sampling = sampling & 0x0F;
        let quant_table_id = data[p + 2];
        p += 3;

        if h_sampling == 0 || v_sampling == 0 || h_sampling > 4 || v_sampling > 4 {
            return Err(format!(
                "Invalid sampling factor {}x{} for component {}",
                h_sampling, v_sampling, id
            )
            .into());
        }

        components.push(Component {
            id,
            h_sampling,
            v_sampling,
            quant_table_id,
        });
    }

    ctx.frame = Some(FrameHeader {
        width,
        height,
        components,
    });

    Ok(end)
}

fn parse_dht(data: &[u8], pos: usize, ctx: &mut JpegContext) -> Result<usize, Box<dyn Error>> {
    let seg_len = read_u16_be(data, pos)? as usize;
    let end = pos + seg_len;
    let mut p = pos + 2;

    while p < end {
        let class_id = data[p];
        let table_class = class_id >> 4; // 0 = DC, 1 = AC
        let table_id = (class_id & 0x0F) as usize;
        p += 1;

        if table_id > 3 {
            return Err(format!("Invalid Huffman table ID: {}", table_id).into());
        }

        let mut bits = [0u8; 16];
        bits.copy_from_slice(&data[p..p + 16]);
        p += 16;

        let total_values: usize = bits.iter().map(|&b| b as usize).sum();
        let values = &data[p..p + total_values];
        p += total_values;

        let table = build_huffman_decode_table(&bits, values);

        if table_class == 0 {
            ctx.dc_huff_tables[table_id] = Some(table);
        } else {
            ctx.ac_huff_tables[table_id] = Some(table);
        }
    }

    Ok(end)
}

fn parse_dri(data: &[u8], pos: usize, ctx: &mut JpegContext) -> Result<usize, Box<dyn Error>> {
    let seg_len = read_u16_be(data, pos)? as usize;
    let end = pos + seg_len;
    ctx.restart_interval = read_u16_be(data, pos + 2)?;
    Ok(end)
}

fn parse_sos(
    data: &[u8],
    pos: usize,
    ctx: &JpegContext,
) -> Result<(Vec<ScanComponentSelector>, usize), Box<dyn Error>> {
    let seg_len = read_u16_be(data, pos)? as usize;
    let mut p = pos + 2;

    let num_components = data[p] as usize;
    p += 1;

    let frame = ctx.frame.as_ref().ok_or("SOS before SOF0")?;

    let mut selectors = Vec::with_capacity(num_components);
    for _ in 0..num_components {
        let comp_id = data[p];
        let table_sel = data[p + 1];
        p += 2;

        let component_index = frame
            .components
            .iter()
            .position(|c| c.id == comp_id)
            .ok_or_else(|| format!("SOS references unknown component ID {}", comp_id))?;

        selectors.push(ScanComponentSelector {
            component_index,
            dc_table_id: table_sel >> 4,
            ac_table_id: table_sel & 0x0F,
        });
    }

    // Skip Ss, Se, Ah/Al (spectral selection / successive approximation)
    // p += 3;
    let entropy_start = pos + seg_len;

    Ok((selectors, entropy_start))
}

// ---- Entropy decoding + reconstruction ----

/// Decode a DC coefficient value from the Huffman category + extra bits.
fn decode_dc_value(category: u8, reader: &mut JpegBitReader) -> Result<i32, Box<dyn Error>> {
    if category == 0 {
        return Ok(0);
    }
    let extra = reader.read_bits(category)? as i32;
    // If MSB of extra bits is 0, value is negative (one's complement encoding)
    let threshold = 1 << (category - 1);
    if extra < threshold {
        Ok(extra - (1 << category) + 1)
    } else {
        Ok(extra)
    }
}

fn decode_scan(
    data: &[u8],
    entropy_start: usize,
    frame: &FrameHeader,
    selectors: &[ScanComponentSelector],
    ctx: &JpegContext,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let width = frame.width as usize;
    let height = frame.height as usize;

    // Determine max sampling factors
    let max_h = frame.components.iter().map(|c| c.h_sampling).max().unwrap_or(1);
    let max_v = frame.components.iter().map(|c| c.v_sampling).max().unwrap_or(1);

    // MCU dimensions in pixels
    let mcu_px_w = max_h as usize * 8;
    let mcu_px_h = max_v as usize * 8;

    // Number of MCUs
    let mcu_cols = (width + mcu_px_w - 1) / mcu_px_w;
    let mcu_rows = (height + mcu_px_h - 1) / mcu_px_h;

    // Allocate component planes at full MCU-aligned resolution
    let plane_w: Vec<usize> = frame
        .components
        .iter()
        .map(|c| mcu_cols * c.h_sampling as usize * 8)
        .collect();
    let plane_h: Vec<usize> = frame
        .components
        .iter()
        .map(|c| mcu_rows * c.v_sampling as usize * 8)
        .collect();
    let mut planes: Vec<Vec<u8>> = frame
        .components
        .iter()
        .enumerate()
        .map(|(i, _)| vec![128u8; plane_w[i] * plane_h[i]])
        .collect();

    // DC predictors (one per component)
    let mut dc_pred = vec![0i32; frame.components.len()];

    let mut reader = JpegBitReader::new(&data[entropy_start..]);
    let mut mcu_count = 0u32;

    for mcu_row in 0..mcu_rows {
        for mcu_col in 0..mcu_cols {
            // Check restart interval
            if ctx.restart_interval > 0 && mcu_count > 0 && mcu_count % ctx.restart_interval as u32 == 0 {
                reader.align();
                // Try to skip to the next restart marker
                let _ = reader.skip_to_restart();
                // Reset DC predictors
                for pred in dc_pred.iter_mut() {
                    *pred = 0;
                }
            }

            // Decode each component's blocks in this MCU
            for sel in selectors {
                let ci = sel.component_index;
                let comp = &frame.components[ci];
                let dc_table = ctx.dc_huff_tables[sel.dc_table_id as usize]
                    .as_ref()
                    .ok_or_else(|| format!("Missing DC Huffman table {}", sel.dc_table_id))?;
                let ac_table = ctx.ac_huff_tables[sel.ac_table_id as usize]
                    .as_ref()
                    .ok_or_else(|| format!("Missing AC Huffman table {}", sel.ac_table_id))?;
                let qt = &ctx.quant_tables[comp.quant_table_id as usize];

                for v_block in 0..comp.v_sampling as usize {
                    for h_block in 0..comp.h_sampling as usize {
                        let mut coeffs = [0i32; 64];

                        // DC coefficient
                        let dc_category = dc_table.decode(&mut reader)?;
                        let dc_diff = decode_dc_value(dc_category, &mut reader)?;
                        dc_pred[ci] += dc_diff;
                        coeffs[0] = dc_pred[ci];

                        // AC coefficients (zigzag positions 1..63)
                        let mut k = 1usize;
                        while k < 64 {
                            let symbol = ac_table.decode(&mut reader)?;
                            if symbol == 0x00 {
                                // EOB: remaining coefficients are zero
                                break;
                            }
                            let run = (symbol >> 4) as usize;
                            let category = symbol & 0x0F;

                            if symbol == 0xF0 {
                                // ZRL: skip 16 zeros
                                k += 16;
                                continue;
                            }

                            k += run;
                            if k >= 64 {
                                break;
                            }

                            let value = decode_dc_value(category, &mut reader)?;
                            // coeffs is in zigzag order at this point
                            coeffs[k] = value;
                            k += 1;
                        }

                        // Dezigzag: reorder from zigzag to natural 8x8 order
                        let mut natural = [0i32; 64];
                        for zz in 0..64 {
                            natural[DEZIGZAG[zz]] = coeffs[zz];
                        }

                        // Dequantize
                        dequantize_block(&mut natural, qt);

                        // Inverse DCT -> pixel values [0, 255]
                        idct8x8(&mut natural);

                        // Write block into component plane
                        let block_x = mcu_col * comp.h_sampling as usize * 8 + h_block * 8;
                        let block_y = mcu_row * comp.v_sampling as usize * 8 + v_block * 8;
                        let pw = plane_w[ci];

                        for row in 0..8 {
                            let dst_y = block_y + row;
                            for col in 0..8 {
                                let dst_x = block_x + col;
                                planes[ci][dst_y * pw + dst_x] = natural[row * 8 + col] as u8;
                            }
                        }
                    }
                }
            }

            mcu_count += 1;
        }
    }

    // Assemble RGB output
    if frame.components.len() == 1 {
        // Grayscale
        return assemble_grayscale(&planes[0], plane_w[0], width, height);
    }

    if frame.components.len() < 3 {
        return Err(format!(
            "Unsupported number of components: {}",
            frame.components.len()
        )
        .into());
    }

    // Find Y, Cb, Cr component indices from scan selectors
    // Typically component IDs: 1=Y, 2=Cb, 3=Cr (or 0=Y, 1=Cb, 2=Cr)
    let y_idx = 0;
    let cb_idx = 1;
    let cr_idx = 2;

    assemble_ycbcr_to_rgb(
        &planes,
        &plane_w,
        &plane_h,
        &frame.components,
        y_idx,
        cb_idx,
        cr_idx,
        max_h,
        max_v,
        width,
        height,
    )
}

fn assemble_grayscale(
    y_plane: &[u8],
    y_stride: usize,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut rgb = vec![0u8; width * height * 3];
    for row in 0..height {
        for col in 0..width {
            let y = y_plane[row * y_stride + col];
            let dst = (row * width + col) * 3;
            rgb[dst] = y;
            rgb[dst + 1] = y;
            rgb[dst + 2] = y;
        }
    }
    Ok(rgb)
}

fn assemble_ycbcr_to_rgb(
    planes: &[Vec<u8>],
    plane_w: &[usize],
    plane_h: &[usize],
    components: &[Component],
    y_idx: usize,
    cb_idx: usize,
    cr_idx: usize,
    max_h: u8,
    max_v: u8,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut rgb = vec![0u8; width * height * 3];

    let y_plane = &planes[y_idx];
    let cb_plane = &planes[cb_idx];
    let cr_plane = &planes[cr_idx];

    let y_stride = plane_w[y_idx];
    let cb_stride = plane_w[cb_idx];
    let cr_stride = plane_w[cr_idx];

    // Compute the ratio of luma pixels to chroma pixels for each axis
    let cb_h_ratio = components[y_idx].h_sampling / components[cb_idx].h_sampling;
    let cb_v_ratio = components[y_idx].v_sampling / components[cb_idx].v_sampling;
    let cr_h_ratio = components[y_idx].h_sampling / components[cr_idx].h_sampling;
    let cr_v_ratio = components[y_idx].v_sampling / components[cr_idx].v_sampling;

    let _ = (plane_h, max_h, max_v); // used for plane allocation

    for row in 0..height {
        for col in 0..width {
            let y_val = y_plane[row * y_stride + col] as f32;

            // Map luma pixel to chroma pixel via nearest-neighbor
            let cb_col = col / cb_h_ratio as usize;
            let cb_row = row / cb_v_ratio as usize;
            let cr_col = col / cr_h_ratio as usize;
            let cr_row = row / cr_v_ratio as usize;

            let cb_val = cb_plane[cb_row * cb_stride + cb_col] as f32 - 128.0;
            let cr_val = cr_plane[cr_row * cr_stride + cr_col] as f32 - 128.0;

            let r = (y_val + 1.402 * cr_val).round().clamp(0.0, 255.0) as u8;
            let g = (y_val - 0.34414 * cb_val - 0.71414 * cr_val)
                .round()
                .clamp(0.0, 255.0) as u8;
            let b = (y_val + 1.772 * cb_val).round().clamp(0.0, 255.0) as u8;

            let dst = (row * width + col) * 3;
            rgb[dst] = r;
            rgb[dst + 1] = g;
            rgb[dst + 2] = b;
        }
    }

    Ok(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_own_encoded_4x4() {
        // Encode a small solid-color image and decode it back
        let rgb_in = vec![200u8, 100, 50].repeat(4 * 4);
        let jpeg_data =
            crate::codec::jpeg::encode_jpeg(&rgb_in, 4, 4, 95).expect("encode should succeed");

        let (rgb_out, w, h) = decode_jpeg(&jpeg_data).expect("decode should succeed");
        assert_eq!(w, 4);
        assert_eq!(h, 4);
        assert_eq!(rgb_out.len(), 4 * 4 * 3);

        // Allow lossy tolerance (JPEG is lossy)
        for i in 0..rgb_out.len() {
            let diff = (rgb_in[i] as i32 - rgb_out[i] as i32).abs();
            assert!(
                diff < 30,
                "pixel byte {} differs by {} (in={}, out={})",
                i,
                diff,
                rgb_in[i],
                rgb_out[i]
            );
        }
    }

    #[test]
    fn decode_own_encoded_32x32_gradient() {
        let (width, height) = (32usize, 32usize);
        let mut rgb_in = vec![0u8; width * height * 3];
        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) * 3;
                rgb_in[idx] = (x * 255 / (width - 1)) as u8;
                rgb_in[idx + 1] = (y * 255 / (height - 1)) as u8;
                rgb_in[idx + 2] = 128;
            }
        }

        let jpeg_data =
            crate::codec::jpeg::encode_jpeg(&rgb_in, width as u32, height as u32, 90)
                .expect("encode should succeed");

        let (rgb_out, w, h) = decode_jpeg(&jpeg_data).expect("decode should succeed");
        assert_eq!(w, width as u32);
        assert_eq!(h, height as u32);

        // Compute PSNR -- should be reasonable for quality 90
        let mse: f64 = rgb_in
            .iter()
            .zip(rgb_out.iter())
            .map(|(&a, &b)| {
                let d = a as f64 - b as f64;
                d * d
            })
            .sum::<f64>()
            / rgb_in.len() as f64;

        let psnr = if mse < 0.001 {
            100.0
        } else {
            10.0 * (255.0_f64 * 255.0 / mse).log10()
        };

        assert!(
            psnr > 15.0,
            "PSNR {:.1}dB is too low for quality 90 roundtrip",
            psnr
        );
    }

    #[test]
    fn decode_rejects_non_jpeg() {
        let result = decode_jpeg(&[0x00, 0x00, 0x00]);
        assert!(result.is_err());
    }

    #[test]
    fn decode_non_multiple_of_8() {
        // 13x7 non-aligned dimensions
        let rgb_in = vec![100u8; 13 * 7 * 3];
        let jpeg_data =
            crate::codec::jpeg::encode_jpeg(&rgb_in, 13, 7, 75).expect("encode should succeed");

        let (rgb_out, w, h) = decode_jpeg(&jpeg_data).expect("decode should succeed");
        assert_eq!(w, 13);
        assert_eq!(h, 7);
        assert_eq!(rgb_out.len(), 13 * 7 * 3);
    }

    #[test]
    fn decode_1x1_pixel() {
        let rgb_in = vec![255u8, 0, 0];
        let jpeg_data =
            crate::codec::jpeg::encode_jpeg(&rgb_in, 1, 1, 75).expect("encode should succeed");

        let (rgb_out, w, h) = decode_jpeg(&jpeg_data).expect("decode should succeed");
        assert_eq!(w, 1);
        assert_eq!(h, 1);
        assert_eq!(rgb_out.len(), 3);
    }

    #[test]
    fn decode_image_crate_encoded_jpeg() {
        // Produce a JPEG using the `image` crate (third-party encoder) and decode with ours
        use image::{ImageEncoder, codecs::jpeg::JpegEncoder};
        let (width, height) = (48u32, 32u32);
        let mut rgb = vec![0u8; (width * height * 3) as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let idx = (y * width as usize + x) * 3;
                rgb[idx] = (x * 255 / 47) as u8;
                rgb[idx + 1] = (y * 255 / 31) as u8;
                rgb[idx + 2] = 100;
            }
        }

        let mut jpeg_buf = Vec::new();
        {
            let enc = JpegEncoder::new_with_quality(&mut jpeg_buf, 90);
            enc.write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)
                .expect("image crate encode");
        }

        let (rgb_out, w, h) = decode_jpeg(&jpeg_buf).expect("should decode image-crate JPEG");
        assert_eq!(w, width);
        assert_eq!(h, height);
        assert_eq!(rgb_out.len(), (width * height * 3) as usize);

        // Compute PSNR against original
        let mse: f64 = rgb
            .iter()
            .zip(rgb_out.iter())
            .map(|(&a, &b)| {
                let d = a as f64 - b as f64;
                d * d
            })
            .sum::<f64>()
            / rgb.len() as f64;

        let psnr = if mse < 0.001 {
            100.0
        } else {
            10.0 * (255.0_f64 * 255.0 / mse).log10()
        };

        assert!(
            psnr > 20.0,
            "PSNR {:.1}dB is too low for image-crate JPEG at quality 90",
            psnr
        );
    }

    #[test]
    fn dezigzag_is_inverse_of_zigzag() {
        use super::super::tables::ZIGZAG;
        for natural in 0..64 {
            let zigzag_pos = ZIGZAG[natural];
            assert_eq!(
                DEZIGZAG[zigzag_pos], natural,
                "DEZIGZAG[ZIGZAG[{}]] should be {}, got {}",
                natural, natural, DEZIGZAG[zigzag_pos]
            );
        }
    }
}
