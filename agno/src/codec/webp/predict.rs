fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Compute SAD (Sum of Absolute Differences) between two same-length slices.
fn sad(a: &[u8], b: &[u8]) -> u32 {
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| (x as i32 - y as i32).unsigned_abs())
        .sum()
}

/// 16x16 luma intra prediction. Mode: 0=DC, 1=V, 2=H, 3=TM.
///
/// `above`: 16 pixels from the row above the macroblock.
/// `left`: 16 pixels from the column left of the macroblock.
/// `top_left`: the diagonal neighbor pixel.
/// `has_above` / `has_left`: whether border data is available (false at image edges).
pub fn predict_16x16(
    above: &[u8; 16],
    left: &[u8; 16],
    top_left: u8,
    mode: u8,
    has_above: bool,
    has_left: bool,
) -> [u8; 256] {
    let mut out = [0u8; 256];
    match mode {
        // DC
        0 => {
            let dc = compute_dc_16(above, left, has_above, has_left);
            out.fill(dc);
        }
        // V (vertical): replicate the top row into every row
        1 => {
            for r in 0..16 {
                out[r * 16..r * 16 + 16].copy_from_slice(above);
            }
        }
        // H (horizontal): replicate the left column into every column
        2 => {
            for r in 0..16 {
                out[r * 16..r * 16 + 16].fill(left[r]);
            }
        }
        // TM (TrueMotion): pixel[r][c] = clamp(left[r] + above[c] - top_left)
        3 => {
            let tl = top_left as i32;
            for r in 0..16 {
                let l = left[r] as i32;
                for c in 0..16 {
                    out[r * 16 + c] = clamp_u8(l + above[c] as i32 - tl);
                }
            }
        }
        _ => out.fill(128),
    }
    out
}

/// 8x8 chroma intra prediction. Mode: 0=DC, 1=V, 2=H, 3=TM.
pub fn predict_chroma_8x8(
    above: &[u8; 8],
    left: &[u8; 8],
    top_left: u8,
    mode: u8,
    has_above: bool,
    has_left: bool,
) -> [u8; 64] {
    let mut out = [0u8; 64];
    match mode {
        0 => {
            let dc = compute_dc_8(above, left, has_above, has_left);
            out.fill(dc);
        }
        1 => {
            for r in 0..8 {
                out[r * 8..r * 8 + 8].copy_from_slice(above);
            }
        }
        2 => {
            for r in 0..8 {
                out[r * 8..r * 8 + 8].fill(left[r]);
            }
        }
        3 => {
            let tl = top_left as i32;
            for r in 0..8 {
                let l = left[r] as i32;
                for c in 0..8 {
                    out[r * 8 + c] = clamp_u8(l + above[c] as i32 - tl);
                }
            }
        }
        _ => out.fill(128),
    }
    out
}

/// Returns the best 16x16 intra mode (lowest SAD) for a luma macroblock.
///
/// `pixels`: the full Y plane.
/// `stride`: row stride of the Y plane.
/// `mb_row` / `mb_col`: macroblock grid position.
pub fn best_16x16_mode(
    pixels: &[u8],
    stride: usize,
    mb_row: usize,
    mb_col: usize,
    above: &[u8; 16],
    left: &[u8; 16],
    top_left: u8,
) -> u8 {
    let has_above = mb_row > 0;
    let has_left = mb_col > 0;

    // Extract the actual 16x16 pixel block from the image.
    let mut block = [0u8; 256];
    let base_y = mb_row * 16;
    let base_x = mb_col * 16;
    for r in 0..16 {
        let src_off = (base_y + r) * stride + base_x;
        let dst_off = r * 16;
        block[dst_off..dst_off + 16].copy_from_slice(&pixels[src_off..src_off + 16]);
    }

    let mut best_mode = 0u8;
    let mut best_sad = u32::MAX;

    for mode in 0..4u8 {
        // Skip V mode if no above row, skip H mode if no left column.
        if mode == 1 && !has_above {
            continue;
        }
        if mode == 2 && !has_left {
            continue;
        }
        if mode == 3 && (!has_above || !has_left) {
            continue;
        }

        let pred = predict_16x16(above, left, top_left, mode, has_above, has_left);
        let s = sad(&block, &pred);
        if s < best_sad {
            best_sad = s;
            best_mode = mode;
        }
    }

    best_mode
}

/// Returns the best 8x8 intra mode (lowest SAD) for a chroma macroblock.
pub fn best_chroma_mode(
    pixels: &[u8],
    stride: usize,
    mb_row: usize,
    mb_col: usize,
    above: &[u8; 8],
    left: &[u8; 8],
    top_left: u8,
) -> u8 {
    let has_above = mb_row > 0;
    let has_left = mb_col > 0;

    let mut block = [0u8; 64];
    let base_y = mb_row * 8;
    let base_x = mb_col * 8;
    for r in 0..8 {
        let src_off = (base_y + r) * stride + base_x;
        let dst_off = r * 8;
        block[dst_off..dst_off + 8].copy_from_slice(&pixels[src_off..src_off + 8]);
    }

    let mut best_mode = 0u8;
    let mut best_sad = u32::MAX;

    for mode in 0..4u8 {
        if mode == 1 && !has_above {
            continue;
        }
        if mode == 2 && !has_left {
            continue;
        }
        if mode == 3 && (!has_above || !has_left) {
            continue;
        }

        let pred = predict_chroma_8x8(above, left, top_left, mode, has_above, has_left);
        let s = sad(&block, &pred);
        if s < best_sad {
            best_sad = s;
            best_mode = mode;
        }
    }

    best_mode
}

fn compute_dc_16(above: &[u8; 16], left: &[u8; 16], has_above: bool, has_left: bool) -> u8 {
    match (has_above, has_left) {
        (true, true) => {
            let sum: u32 = above.iter().map(|&x| x as u32).sum::<u32>()
                + left.iter().map(|&x| x as u32).sum::<u32>();
            ((sum + 16) / 32) as u8
        }
        (true, false) => {
            let sum: u32 = above.iter().map(|&x| x as u32).sum();
            ((sum + 8) / 16) as u8
        }
        (false, true) => {
            let sum: u32 = left.iter().map(|&x| x as u32).sum();
            ((sum + 8) / 16) as u8
        }
        (false, false) => 128,
    }
}

fn compute_dc_8(above: &[u8; 8], left: &[u8; 8], has_above: bool, has_left: bool) -> u8 {
    match (has_above, has_left) {
        (true, true) => {
            let sum: u32 = above.iter().map(|&x| x as u32).sum::<u32>()
                + left.iter().map(|&x| x as u32).sum::<u32>();
            ((sum + 8) / 16) as u8
        }
        (true, false) => {
            let sum: u32 = above.iter().map(|&x| x as u32).sum();
            ((sum + 4) / 8) as u8
        }
        (false, true) => {
            let sum: u32 = left.iter().map(|&x| x as u32).sum();
            ((sum + 4) / 8) as u8
        }
        (false, false) => 128,
    }
}
