// Forward 8x8 DCT using the IJG "slow integer" approach (jfdctint.c).
// Uses 13-bit fixed-point for high precision, matching the standard JPEG DCT
// so that the roundtrip with any conformant IDCT produces minimal error.

const CONST_BITS: i32 = 13;
const PASS1_BITS: i32 = 2;

// Fixed-point constants (scaled by 2^13 = 8192)
const FIX_0_298631336: i32 = 2446; // cos(7*pi/16) * 8192
const FIX_0_390180644: i32 = 3196;
const FIX_0_541196100: i32 = 4433; // sqrt(2) * cos(3*pi/8) * 8192
const FIX_0_765366865: i32 = 6270; // sqrt(2) * cos(5*pi/16) * 8192
const FIX_0_899976223: i32 = 7373;
const FIX_1_175875602: i32 = 9633; // sqrt(2) * cos(pi/8) * 8192
const FIX_1_501321110: i32 = 12299;
const FIX_1_847759065: i32 = 15137; // sqrt(2) * cos(pi/8) * 8192
const FIX_1_961570560: i32 = 16069;
const FIX_2_053119869: i32 = 16819;
const FIX_2_562915447: i32 = 20995;
const FIX_3_072711026: i32 = 25172;

/// Forward DCT on an 8x8 block (in-place).
/// Input: pixel values 0-255. Output: standard DCT coefficients (level-shifted).
pub fn fdct8x8(block: &mut [i32; 64]) {
    // Level-shift: center around zero
    for v in block.iter_mut() {
        *v -= 128;
    }

    // Pass 1: process rows.
    // Results are scaled up by sqrt(8) * 2^PASS1_BITS = sqrt(8) * 4.
    for row in 0..8 {
        let base = row * 8;
        fdct_row(block, base);
    }

    // Pass 2: process columns.
    // Removes the PASS1_BITS scaling and applies the final normalization.
    for col in 0..8 {
        fdct_col(block, col);
    }
}

fn fdct_row(block: &mut [i32; 64], base: usize) {
    let d0 = block[base];
    let d1 = block[base + 1];
    let d2 = block[base + 2];
    let d3 = block[base + 3];
    let d4 = block[base + 4];
    let d5 = block[base + 5];
    let d6 = block[base + 6];
    let d7 = block[base + 7];

    let tmp0 = d0 + d7;
    let tmp7 = d0 - d7;
    let tmp1 = d1 + d6;
    let tmp6 = d1 - d6;
    let tmp2 = d2 + d5;
    let tmp5 = d2 - d5;
    let tmp3 = d3 + d4;
    let tmp4 = d3 - d4;

    // Even part
    let tmp10 = tmp0 + tmp3;
    let tmp13 = tmp0 - tmp3;
    let tmp11 = tmp1 + tmp2;
    let tmp12 = tmp1 - tmp2;

    block[base] = (tmp10 + tmp11) << PASS1_BITS;
    block[base + 4] = (tmp10 - tmp11) << PASS1_BITS;

    let z1 = (tmp12 + tmp13) * FIX_0_541196100;
    block[base + 2] = (z1 + tmp13 * FIX_0_765366865) >> (CONST_BITS - PASS1_BITS);
    block[base + 6] = (z1 - tmp12 * FIX_1_847759065) >> (CONST_BITS - PASS1_BITS);

    // Odd part — standard IJG formulation
    let z1 = tmp4 + tmp7;
    let z2 = tmp5 + tmp6;
    let z3 = tmp4 + tmp6;
    let z4 = tmp5 + tmp7;
    let z5 = (z3 + z4) * FIX_1_175875602;

    let tmp4 = tmp4 * FIX_0_298631336;
    let tmp5 = tmp5 * FIX_2_053119869;
    let tmp6 = tmp6 * FIX_3_072711026;
    let tmp7 = tmp7 * FIX_1_501321110;
    let z1 = z1 * (-FIX_0_899976223);
    let z2 = z2 * (-FIX_2_562915447);
    let z3 = z3 * (-FIX_1_961570560) + z5;
    let z4 = z4 * (-FIX_0_390180644) + z5;

    block[base + 7] = (tmp4 + z1 + z3) >> (CONST_BITS - PASS1_BITS);
    block[base + 5] = (tmp5 + z2 + z4) >> (CONST_BITS - PASS1_BITS);
    block[base + 3] = (tmp6 + z2 + z3) >> (CONST_BITS - PASS1_BITS);
    block[base + 1] = (tmp7 + z1 + z4) >> (CONST_BITS - PASS1_BITS);
}

fn fdct_col(block: &mut [i32; 64], col: usize) {
    let d0 = block[col];
    let d1 = block[col + 8];
    let d2 = block[col + 16];
    let d3 = block[col + 24];
    let d4 = block[col + 32];
    let d5 = block[col + 40];
    let d6 = block[col + 48];
    let d7 = block[col + 56];

    let tmp0 = d0 + d7;
    let tmp7 = d0 - d7;
    let tmp1 = d1 + d6;
    let tmp6 = d1 - d6;
    let tmp2 = d2 + d5;
    let tmp5 = d2 - d5;
    let tmp3 = d3 + d4;
    let tmp4 = d3 - d4;

    // Even part
    let tmp10 = tmp0 + tmp3;
    let tmp13 = tmp0 - tmp3;
    let tmp11 = tmp1 + tmp2;
    let tmp12 = tmp1 - tmp2;

    // Final right-shift removes PASS1_BITS scaling and normalizes.
    // The total normalization for 2D DCT is 1/8 per dimension = 1/64 total.
    // After pass1 (<<PASS1_BITS=2) and pass2, we need >>( PASS1_BITS + 3 ) = >>5.
    let descale = PASS1_BITS + 3;

    block[col] = (tmp10 + tmp11 + (1 << (descale - 1))) >> descale;
    block[col + 32] = (tmp10 - tmp11 + (1 << (descale - 1))) >> descale;

    let z1 = (tmp12 + tmp13) * FIX_0_541196100;
    block[col + 16] = (z1 + tmp13 * FIX_0_765366865 + (1 << (CONST_BITS + descale - 1)))
        >> (CONST_BITS + descale);
    block[col + 48] = (z1 - tmp12 * FIX_1_847759065 + (1 << (CONST_BITS + descale - 1)))
        >> (CONST_BITS + descale);

    // Odd part — same IJG formulation as row pass
    let z1 = tmp4 + tmp7;
    let z2 = tmp5 + tmp6;
    let z3 = tmp4 + tmp6;
    let z4 = tmp5 + tmp7;
    let z5 = (z3 + z4) * FIX_1_175875602;

    let tmp4 = tmp4 * FIX_0_298631336;
    let tmp5 = tmp5 * FIX_2_053119869;
    let tmp6 = tmp6 * FIX_3_072711026;
    let tmp7 = tmp7 * FIX_1_501321110;
    let z1 = z1 * (-FIX_0_899976223);
    let z2 = z2 * (-FIX_2_562915447);
    let z3 = z3 * (-FIX_1_961570560) + z5;
    let z4 = z4 * (-FIX_0_390180644) + z5;

    let round = 1 << (CONST_BITS + descale - 1);
    block[col + 56] = (tmp4 + z1 + z3 + round) >> (CONST_BITS + descale);
    block[col + 40] = (tmp5 + z2 + z4 + round) >> (CONST_BITS + descale);
    block[col + 24] = (tmp6 + z2 + z3 + round) >> (CONST_BITS + descale);
    block[col + 8] = (tmp7 + z1 + z4 + round) >> (CONST_BITS + descale);
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
        coeffs[i] = if i == 0 { val } else { val.clamp(-1023, 1023) };
    }
}
