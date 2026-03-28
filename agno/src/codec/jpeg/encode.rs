use std::error::Error;
use std::io::Write;

use rayon::prelude::*;

use super::dct::{fdct8x8, quantize_block};
use super::huffman::JpegBitWriter;
use super::tables::*;

/// Encode RGB data as baseline JPEG with 4:2:0 chroma subsampling.
/// Tries GPU first if available and image is large enough, falls back to CPU.
pub fn encode_jpeg(
    rgb: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>, Box<dyn Error>> {
    #[cfg(feature = "gpu")]
    {
        if width >= 256
            && height >= 256
            && let Some(result) = crate::jpeg_gpu::encode_jpeg_gpu(rgb, width, height, quality)
        {
            return Ok(result);
        }
    }
    encode_jpeg_cpu(rgb, width, height, quality)
}

/// Pure CPU baseline JPEG encoder.
fn encode_jpeg_cpu(
    rgb: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let w = width as usize;
    let h = height as usize;

    // Convert RGB to separate Y, Cb, Cr planes
    let num_pixels = w * h;
    let mut y_plane = vec![0u8; num_pixels];
    let mut cb_plane = vec![0u8; num_pixels];
    let mut cr_plane = vec![0u8; num_pixels];

    for i in 0..num_pixels {
        let r = rgb[i * 3] as f32;
        let g = rgb[i * 3 + 1] as f32;
        let b = rgb[i * 3 + 2] as f32;

        y_plane[i] = (0.299 * r + 0.587 * g + 0.114 * b)
            .round()
            .clamp(0.0, 255.0) as u8;
        cb_plane[i] = (128.0 - 0.168736 * r - 0.331264 * g + 0.5 * b)
            .round()
            .clamp(0.0, 255.0) as u8;
        cr_plane[i] = (128.0 + 0.5 * r - 0.418688 * g - 0.081312 * b)
            .round()
            .clamp(0.0, 255.0) as u8;
    }

    encode_jpeg_from_ycbcr(&y_plane, &cb_plane, &cr_plane, width, height, quality)
}

/// Encode pre-converted YCbCr planes as baseline JPEG with 4:2:0 subsampling.
/// Used by both the CPU path (after RGB->YCbCr conversion) and the GPU path
/// (where color conversion is done on the GPU).
#[allow(clippy::needless_range_loop)]
pub fn encode_jpeg_from_ycbcr(
    y_plane: &[u8],
    cb_plane: &[u8],
    cr_plane: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let w = width as usize;
    let h = height as usize;
    let quality = quality.clamp(1, 100);

    let luma_qt = scale_quant_table(&JPEG_LUMA_QUANT, quality);
    let chroma_qt = scale_quant_table(&JPEG_CHROMA_QUANT, quality);

    // MCU dimensions for 4:2:0 (16x16 pixel blocks)
    let mcu_cols = w.div_ceil(16);
    let mcu_rows = h.div_ceil(16);
    let total_mcus = mcu_cols * mcu_rows;

    // For each MCU: 4 Y blocks + 1 Cb block + 1 Cr block = 6 blocks
    // Each block is [i16; 64] in zigzag order after DCT + quantize
    // Parallelize DCT+quantize per MCU, collect results sequentially.

    let mcu_blocks: Vec<[[i16; 64]; 6]> = (0..total_mcus)
        .into_par_iter()
        .map(|mcu_idx| {
            let mcu_row = mcu_idx / mcu_cols;
            let mcu_col = mcu_idx % mcu_cols;
            let px = mcu_col * 16;
            let py = mcu_row * 16;

            let mut blocks = [[0i16; 64]; 6];

            // Extract and process 4 Y blocks
            for blk in 0..4 {
                let bx = px + (blk % 2) * 8;
                let by = py + (blk / 2) * 8;
                let mut dct_block = [0i32; 64];
                for row in 0..8 {
                    for col in 0..8 {
                        let sx = (bx + col).min(w - 1);
                        let sy = (by + row).min(h - 1);
                        dct_block[row * 8 + col] = y_plane[sy * w + sx] as i32;
                    }
                }
                fdct8x8(&mut dct_block);
                quantize_block(&mut dct_block, &luma_qt);
                blocks[blk] = zigzag_reorder(&dct_block);
            }

            // Subsample Cb and Cr: average 2x2 groups from the 16x16 MCU region
            let mut cb_block = [0i32; 64];
            let mut cr_block = [0i32; 64];
            for row in 0..8 {
                for col in 0..8 {
                    let sx0 = (px + col * 2).min(w - 1);
                    let sy0 = (py + row * 2).min(h - 1);
                    let sx1 = (px + col * 2 + 1).min(w - 1);
                    let sy1 = (py + row * 2 + 1).min(h - 1);

                    let cb_sum = cb_plane[sy0 * w + sx0] as u32
                        + cb_plane[sy0 * w + sx1] as u32
                        + cb_plane[sy1 * w + sx0] as u32
                        + cb_plane[sy1 * w + sx1] as u32;
                    cb_block[row * 8 + col] = ((cb_sum + 2) / 4) as i32;

                    let cr_sum = cr_plane[sy0 * w + sx0] as u32
                        + cr_plane[sy0 * w + sx1] as u32
                        + cr_plane[sy1 * w + sx0] as u32
                        + cr_plane[sy1 * w + sx1] as u32;
                    cr_block[row * 8 + col] = ((cr_sum + 2) / 4) as i32;
                }
            }

            fdct8x8(&mut cb_block);
            quantize_block(&mut cb_block, &chroma_qt);
            blocks[4] = zigzag_reorder(&cb_block);

            fdct8x8(&mut cr_block);
            quantize_block(&mut cr_block, &chroma_qt);
            blocks[5] = zigzag_reorder(&cr_block);

            blocks
        })
        .collect();

    // Assemble the JPEG bitstream
    let mut out = Vec::with_capacity(w * h); // rough estimate

    // SOI
    out.write_all(&[0xFF, 0xD8])?;

    // APP0 (JFIF)
    write_app0(&mut out)?;

    // DQT markers
    write_dqt(&mut out, 0, &luma_qt)?;
    write_dqt(&mut out, 1, &chroma_qt)?;

    // SOF0
    write_sof0(&mut out, width, height)?;

    // DHT markers (4 tables)
    write_dht(&mut out, 0x00, &DC_LUMA_BITS, &DC_LUMA_VALUES)?;
    write_dht(&mut out, 0x10, &AC_LUMA_BITS, &AC_LUMA_VALUES)?;
    write_dht(&mut out, 0x01, &DC_CHROMA_BITS, &DC_CHROMA_VALUES)?;
    write_dht(&mut out, 0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALUES)?;

    // SOS
    write_sos(&mut out)?;

    // Entropy-coded data
    {
        let mut bit_writer = JpegBitWriter::new(&mut out);

        let mut prev_dc_y: i16 = 0;
        let mut prev_dc_cb: i16 = 0;
        let mut prev_dc_cr: i16 = 0;

        let dc_luma = &*DC_LUMA_LUT;
        let ac_luma = &*AC_LUMA_LUT;
        let dc_chroma = &*DC_CHROMA_LUT;
        let ac_chroma = &*AC_CHROMA_LUT;

        for blocks in &mcu_blocks {
            // 4 Y blocks
            for block in &blocks[..4] {
                let dc = block[0];
                let diff = dc - prev_dc_y;
                prev_dc_y = dc;
                bit_writer.write_dc(diff, dc_luma)?;
                let ac: [i16; 63] = block[1..64].try_into().unwrap();
                bit_writer.write_ac_block(&ac, ac_luma)?;
            }

            // Cb block
            {
                let dc = blocks[4][0];
                let diff = dc - prev_dc_cb;
                prev_dc_cb = dc;
                bit_writer.write_dc(diff, dc_chroma)?;
                let ac: [i16; 63] = blocks[4][1..64].try_into().unwrap();
                bit_writer.write_ac_block(&ac, ac_chroma)?;
            }

            // Cr block
            {
                let dc = blocks[5][0];
                let diff = dc - prev_dc_cr;
                prev_dc_cr = dc;
                bit_writer.write_dc(diff, dc_chroma)?;
                let ac: [i16; 63] = blocks[5][1..64].try_into().unwrap();
                bit_writer.write_ac_block(&ac, ac_chroma)?;
            }
        }

        bit_writer.flush()?;
    }

    // EOI
    out.write_all(&[0xFF, 0xD9])?;

    Ok(out)
}

/// Reorder DCT coefficients from natural (row-major) order to zigzag scan order.
/// ZIGZAG[scan_pos] = natural_idx, so out[scan_pos] = block[natural_idx].
fn zigzag_reorder(block: &[i32; 64]) -> [i16; 64] {
    let mut out = [0i16; 64];
    for (scan_pos, &natural_idx) in ZIGZAG.iter().enumerate() {
        out[scan_pos] = block[natural_idx] as i16;
    }
    out
}

// --- Marker writing helpers ---

fn write_app0(out: &mut Vec<u8>) -> Result<(), Box<dyn Error>> {
    out.write_all(&[0xFF, 0xE0])?;
    let length: u16 = 16;
    out.write_all(&length.to_be_bytes())?;
    out.write_all(b"JFIF\0")?; // identifier
    out.write_all(&[1, 1])?; // version 1.1
    out.write_all(&[0])?; // aspect ratio units: 0 = no units
    out.write_all(&1u16.to_be_bytes())?; // X density
    out.write_all(&1u16.to_be_bytes())?; // Y density
    out.write_all(&[0, 0])?; // no thumbnail
    Ok(())
}

fn write_dqt(out: &mut Vec<u8>, table_id: u8, qtable: &[u16; 64]) -> Result<(), Box<dyn Error>> {
    out.write_all(&[0xFF, 0xDB])?;
    let length: u16 = 2 + 1 + 64; // length field + Pq/Tq byte + 64 values
    out.write_all(&length.to_be_bytes())?;
    out.write_all(&[table_id])?; // 0 = 8-bit precision, low nibble = table id

    // Write quantization values in zigzag scan order
    let mut zigzag_qt = [0u8; 64];
    for (scan_pos, &natural_idx) in ZIGZAG.iter().enumerate() {
        zigzag_qt[scan_pos] = qtable[natural_idx] as u8;
    }
    out.write_all(&zigzag_qt)?;
    Ok(())
}

fn write_sof0(out: &mut Vec<u8>, width: u32, height: u32) -> Result<(), Box<dyn Error>> {
    out.write_all(&[0xFF, 0xC0])?;
    let length: u16 = 17;
    out.write_all(&length.to_be_bytes())?;
    out.write_all(&[8])?; // 8-bit precision
    out.write_all(&(height as u16).to_be_bytes())?;
    out.write_all(&(width as u16).to_be_bytes())?;
    out.write_all(&[3])?; // 3 components

    // Component 1 (Y): id=1, H=2 V=2, Tq=0
    out.write_all(&[1, 0x22, 0])?;
    // Component 2 (Cb): id=2, H=1 V=1, Tq=1
    out.write_all(&[2, 0x11, 1])?;
    // Component 3 (Cr): id=3, H=1 V=1, Tq=1
    out.write_all(&[3, 0x11, 1])?;
    Ok(())
}

fn write_dht(
    out: &mut Vec<u8>,
    class_id: u8,
    bits: &[u8; 16],
    values: &[u8],
) -> Result<(), Box<dyn Error>> {
    out.write_all(&[0xFF, 0xC4])?;
    let length: u16 = 2 + 1 + 16 + values.len() as u16;
    out.write_all(&length.to_be_bytes())?;
    out.write_all(&[class_id])?; // upper nibble = class (0=DC, 1=AC), lower = id
    out.write_all(bits)?;
    out.write_all(values)?;
    Ok(())
}

fn write_sos(out: &mut Vec<u8>) -> Result<(), Box<dyn Error>> {
    out.write_all(&[0xFF, 0xDA])?;
    let length: u16 = 12;
    out.write_all(&length.to_be_bytes())?;
    out.write_all(&[3])?; // 3 components

    // Component 1 (Y): Td=0, Ta=0
    out.write_all(&[1, 0x00])?;
    // Component 2 (Cb): Td=1, Ta=1
    out.write_all(&[2, 0x11])?;
    // Component 3 (Cr): Td=1, Ta=1
    out.write_all(&[3, 0x11])?;

    // Spectral selection start, end, successive approximation
    out.write_all(&[0, 63, 0])?;
    Ok(())
}
