// AAN (Arai, Agui, Nakajima) fast forward 8x8 DCT
// Fixed-point arithmetic with 8-bit fractional precision.

const FIX_0_707106781: i32 = 181; // cos(pi/4) * 256
const FIX_0_382683433: i32 = 98;  // sin(pi/8) * 256
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

    let z1 = (t12 + t13) * FIX_0_707106781 >> SHIFT;
    block[base + 2] = t13 + z1;
    block[base + 6] = t13 - z1;

    // Odd part
    let t10 = t4 + t5;
    let t11 = t5 + t6;
    let t12 = t6 + t7;

    let z5 = (t10 - t12) * FIX_0_382683433 >> SHIFT;
    let z2 = (FIX_0_541196100 * t10 >> SHIFT) + z5;
    let z4 = (FIX_1_306562965 * t12 >> SHIFT) + z5;
    let z3 = t11 * FIX_0_707106781 >> SHIFT;

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

    let z1 = (t12 + t13) * FIX_0_707106781 >> SHIFT;
    block[col + 16] = t13 + z1;
    block[col + 48] = t13 - z1;

    // Odd part
    let t10 = t4 + t5;
    let t11 = t5 + t6;
    let t12 = t6 + t7;

    let z5 = (t10 - t12) * FIX_0_382683433 >> SHIFT;
    let z2 = (FIX_0_541196100 * t10 >> SHIFT) + z5;
    let z4 = (FIX_1_306562965 * t12 >> SHIFT) + z5;
    let z3 = t11 * FIX_0_707106781 >> SHIFT;

    let z11 = t7 + z3;
    let z13 = t7 - z3;

    block[col + 40] = z13 + z2;
    block[col + 24] = z13 - z2;
    block[col + 8] = z11 + z4;
    block[col + 56] = z11 - z4;
}

/// Quantize DCT coefficients in-place using the given table.
/// Uses rounding division toward zero.
pub fn quantize_block(coeffs: &mut [i32; 64], qtable: &[u16; 64]) {
    for i in 0..64 {
        let q = qtable[i] as i32;
        let half = q / 2;
        coeffs[i] = if coeffs[i] >= 0 {
            (coeffs[i] + half) / q
        } else {
            (coeffs[i] - half) / q
        };
    }
}

// --- Inverse DCT for JPEG decoding ---

// Fixed-point constants for the reference IDCT (12-bit precision).
// W_n = 2048 * sqrt(2) * cos(n * pi / 16), rounded to nearest integer.
const W1: i32 = 2841;
const W2: i32 = 2676;
const W3: i32 = 2408;
const W5: i32 = 1609;
const W6: i32 = 1108;
const W7: i32 = 565;

/// Inverse DCT on an 8x8 block (in-place).
///
/// Input: dequantized DCT coefficients in natural (row-major) order.
/// Output: pixel values in [0, 255] after level shift.
///
/// Uses a separable approach: 1D IDCT on rows, then 1D IDCT on columns,
/// with fixed-point arithmetic matching the classic Chen/Loeffler algorithm.
pub fn idct8x8(block: &mut [i32; 64]) {
    for i in 0..8 {
        idct_row(&mut block[i * 8..(i + 1) * 8]);
    }
    for i in 0..8 {
        idct_col(block, i);
    }
    for v in block.iter_mut() {
        // The row pass scales by 2^3 (<<11, >>8) and the column pass descales by 2^6 (<<8, >>14).
        // Net effect is >>3, which exactly inverts the forward DCT's >>3 normalization.
        // No additional shift needed -- just add the level shift (+128) and clamp.
        *v = (*v + 128).clamp(0, 255);
    }
}

fn idct_row(blk: &mut [i32]) {
    // Even part
    let x0 = (blk[0] << 11) + 128; // +128 for rounding in the final >>8
    let x1 = blk[4] << 11;
    let x2 = blk[6];
    let x3 = blk[2];
    let x4 = blk[1];
    let x5 = blk[7];
    let x6 = blk[5];
    let x7 = blk[3];

    // Odd part butterfly
    let x8 = W7 * (x4 + x5);
    let x4 = x8 + (W1 - W7) * x4;
    let x5 = x8 - (W1 + W7) * x5;
    let x8 = W3 * (x6 + x7);
    let x6 = x8 - (W3 - W5) * x6;
    let x7 = x8 - (W3 + W5) * x7;

    // Even part continued
    let x8 = x0 + x1;
    let x0 = x0 - x1;
    let x1 = W6 * (x3 + x2);
    let x2 = x1 - (W2 + W6) * x2;
    let x3 = x1 + (W2 - W6) * x3;

    // Combine odd
    let x1 = x4 + x6;
    let x4 = x4 - x6;
    let x6 = x5 + x7;
    let x5 = x5 - x7;

    // Final combine
    let x7 = x8 + x3;
    let x8 = x8 - x3;
    let x3 = x0 + x2;
    let x0 = x0 - x2;
    let x2 = (181 * (x4 + x5) + 128) >> 8;
    let x4 = (181 * (x4 - x5) + 128) >> 8;

    blk[0] = (x7 + x1) >> 8;
    blk[1] = (x3 + x2) >> 8;
    blk[2] = (x0 + x4) >> 8;
    blk[3] = (x8 + x6) >> 8;
    blk[4] = (x8 - x6) >> 8;
    blk[5] = (x0 - x4) >> 8;
    blk[6] = (x3 - x2) >> 8;
    blk[7] = (x7 - x1) >> 8;
}

fn idct_col(blk: &mut [i32; 64], col: usize) {
    let x0 = (blk[col] << 8) + 8192; // +8192 for rounding in the final >>14
    let x1 = blk[col + 32] << 8;
    let x2 = blk[col + 48];
    let x3 = blk[col + 16];
    let x4 = blk[col + 8];
    let x5 = blk[col + 56];
    let x6 = blk[col + 40];
    let x7 = blk[col + 24];

    let x8 = W7 * (x4 + x5) + 4;
    let x4 = (x8 + (W1 - W7) * x4) >> 3;
    let x5 = (x8 - (W1 + W7) * x5) >> 3;
    let x8 = W3 * (x6 + x7) + 4;
    let x6 = (x8 - (W3 - W5) * x6) >> 3;
    let x7 = (x8 - (W3 + W5) * x7) >> 3;

    let x8 = x0 + x1;
    let x0 = x0 - x1;
    let x1 = W6 * (x3 + x2) + 4;
    let x2 = (x1 - (W2 + W6) * x2) >> 3;
    let x3 = (x1 + (W2 - W6) * x3) >> 3;

    let x1 = x4 + x6;
    let x4 = x4 - x6;
    let x6 = x5 + x7;
    let x5 = x5 - x7;

    let x7 = x8 + x3;
    let x8 = x8 - x3;
    let x3 = x0 + x2;
    let x0 = x0 - x2;
    let x2 = (181 * (x4 + x5) + 128) >> 8;
    let x4 = (181 * (x4 - x5) + 128) >> 8;

    blk[col] = (x7 + x1) >> 14;
    blk[col + 8] = (x3 + x2) >> 14;
    blk[col + 16] = (x0 + x4) >> 14;
    blk[col + 24] = (x8 + x6) >> 14;
    blk[col + 32] = (x8 - x6) >> 14;
    blk[col + 40] = (x0 - x4) >> 14;
    blk[col + 48] = (x3 - x2) >> 14;
    blk[col + 56] = (x7 - x1) >> 14;
}

/// Dequantize a block: multiply each coefficient by the quantization table value.
pub fn dequantize_block(coeffs: &mut [i32; 64], qtable: &[u16; 64]) {
    for i in 0..64 {
        coeffs[i] *= qtable[i] as i32;
    }
}
