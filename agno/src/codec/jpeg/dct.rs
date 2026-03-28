// AAN (Arai, Agui, Nakajima) fast forward 8x8 DCT
// Fixed-point arithmetic with 8-bit fractional precision.

const FIX_0_707106781: i32 = 181; // cos(pi/4) * 256
const FIX_0_382683433: i32 = 98; // sin(pi/8) * 256
const FIX_0_541196100: i32 = 139;
const FIX_1_306562965: i32 = 334;
const SHIFT: i32 = 8;

/// Forward DCT on an 8x8 block (in-place).
/// Input: pixel values 0-255. Output: DCT coefficients (level-shifted).
pub fn fdct8x8(block: &mut [i32; 64]) {
    // Level-shift: center around zero
    for v in block.iter_mut() {
        *v -= 128;
    }

    // 1D DCT on each row
    for row in 0..8 {
        let base = row * 8;
        fdct_1d_row(block, base);
    }

    // 1D DCT on each column
    for col in 0..8 {
        fdct_1d_col(block, col);
    }

    // Normalization: divide by 8 (right-shift by 3)
    for v in block.iter_mut() {
        *v >>= 3;
    }
}

fn fdct_1d_row(block: &mut [i32; 64], base: usize) {
    let x0 = block[base];
    let x1 = block[base + 1];
    let x2 = block[base + 2];
    let x3 = block[base + 3];
    let x4 = block[base + 4];
    let x5 = block[base + 5];
    let x6 = block[base + 6];
    let x7 = block[base + 7];

    // Butterfly stage
    let t0 = x0 + x7;
    let t7 = x0 - x7;
    let t1 = x1 + x6;
    let t6 = x1 - x6;
    let t2 = x2 + x5;
    let t5 = x2 - x5;
    let t3 = x3 + x4;
    let t4 = x3 - x4;

    // Even part
    let t10 = t0 + t3;
    let t13 = t0 - t3;
    let t11 = t1 + t2;
    let t12 = t1 - t2;

    block[base] = t10 + t11;
    block[base + 4] = t10 - t11;

    let z1 = ((t12 + t13) * FIX_0_707106781) >> SHIFT;
    block[base + 2] = t13 + z1;
    block[base + 6] = t13 - z1;

    // Odd part
    let t10 = t4 + t5;
    let t11 = t5 + t6;
    let t12 = t6 + t7;

    let z5 = ((t10 - t12) * FIX_0_382683433) >> SHIFT;
    let z2 = ((FIX_0_541196100 * t10) >> SHIFT) + z5;
    let z4 = ((FIX_1_306562965 * t12) >> SHIFT) + z5;
    let z3 = (t11 * FIX_0_707106781) >> SHIFT;

    let z11 = t7 + z3;
    let z13 = t7 - z3;

    block[base + 5] = z13 + z2;
    block[base + 3] = z13 - z2;
    block[base + 1] = z11 + z4;
    block[base + 7] = z11 - z4;
}

fn fdct_1d_col(block: &mut [i32; 64], col: usize) {
    let x0 = block[col];
    let x1 = block[col + 8];
    let x2 = block[col + 16];
    let x3 = block[col + 24];
    let x4 = block[col + 32];
    let x5 = block[col + 40];
    let x6 = block[col + 48];
    let x7 = block[col + 56];

    // Butterfly stage
    let t0 = x0 + x7;
    let t7 = x0 - x7;
    let t1 = x1 + x6;
    let t6 = x1 - x6;
    let t2 = x2 + x5;
    let t5 = x2 - x5;
    let t3 = x3 + x4;
    let t4 = x3 - x4;

    // Even part
    let t10 = t0 + t3;
    let t13 = t0 - t3;
    let t11 = t1 + t2;
    let t12 = t1 - t2;

    block[col] = t10 + t11;
    block[col + 32] = t10 - t11;

    let z1 = ((t12 + t13) * FIX_0_707106781) >> SHIFT;
    block[col + 16] = t13 + z1;
    block[col + 48] = t13 - z1;

    // Odd part
    let t10 = t4 + t5;
    let t11 = t5 + t6;
    let t12 = t6 + t7;

    let z5 = ((t10 - t12) * FIX_0_382683433) >> SHIFT;
    let z2 = ((FIX_0_541196100 * t10) >> SHIFT) + z5;
    let z4 = ((FIX_1_306562965 * t12) >> SHIFT) + z5;
    let z3 = (t11 * FIX_0_707106781) >> SHIFT;

    let z11 = t7 + z3;
    let z13 = t7 - z3;

    block[col + 40] = z13 + z2;
    block[col + 24] = z13 - z2;
    block[col + 8] = z11 + z4;
    block[col + 56] = z11 - z4;
}

/// Quantize DCT coefficients in-place using the given table.
/// Uses rounding division toward zero.
/// AC coefficients are clamped to [-1023, 1023] (baseline JPEG category 10 limit).
pub fn quantize_block(coeffs: &mut [i32; 64], qtable: &[u16; 64]) {
    for i in 0..64 {
        let q = qtable[i] as i32;
        let half = q / 2;
        let val = if coeffs[i] >= 0 {
            (coeffs[i] + half) / q
        } else {
            (coeffs[i] - half) / q
        };
        // DC (index 0) can use categories 0-11, but AC coefficients are limited
        // to category 10 (max |value| = 1023) by the standard JPEG Huffman tables.
        coeffs[i] = if i == 0 { val } else { val.clamp(-1023, 1023) };
    }
}
