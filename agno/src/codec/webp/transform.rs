/// Forward 4x4 DCT matching libvpx's `vp8_short_fdct4x4_c`.
///
/// Row pass: sums/differences scaled by ×8 for precision, trig outputs
/// with 2217/5352 constants and >>12 normalization.
/// Column pass: DC/AC0 normalized with >>4, trig outputs with >>16 and
/// different rounding constants than the row pass.
///
/// This forward DCT is the approximate inverse of `idct4x4`, designed so
/// that `idct4x4(fdct4x4(x)) ≈ x` within ±1.
pub fn fdct4x4(input: &[i16; 16]) -> [i16; 16] {
    // Row transform (multiply sums by 8 for precision)
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        let a = (input[i * 4] as i32 + input[i * 4 + 3] as i32) * 8;
        let b = (input[i * 4 + 1] as i32 + input[i * 4 + 2] as i32) * 8;
        let c = (input[i * 4 + 1] as i32 - input[i * 4 + 2] as i32) * 8;
        let d = (input[i * 4] as i32 - input[i * 4 + 3] as i32) * 8;

        tmp[i * 4] = a + b;
        tmp[i * 4 + 1] = (c * 2217 + d * 5352 + 14500) >> 12;
        tmp[i * 4 + 2] = a - b;
        tmp[i * 4 + 3] = (d * 2217 - c * 5352 + 7500) >> 12;
    }

    // Column transform (>>4 normalization for DC, >>16 for trig)
    let mut result = [0i16; 16];
    for i in 0..4 {
        let a = tmp[i] + tmp[i + 12];
        let b = tmp[i + 4] + tmp[i + 8];
        let c = tmp[i + 4] - tmp[i + 8];
        let d = tmp[i] - tmp[i + 12];

        result[i] = ((a + b + 7) >> 4) as i16;
        result[i + 8] = ((a - b + 7) >> 4) as i16;
        result[i + 4] =
            (((c * 2217 + d * 5352 + 12000) >> 16) + i32::from(d != 0)) as i16;
        result[i + 12] = ((d * 2217 - c * 5352 + 51000) >> 16) as i16;
    }
    result
}

/// Forward 4x4 Walsh-Hadamard transform matching libvpx's
/// `vp8_short_walsh4x4_c`.
///
/// Row pass: sums scaled by ×4, each output adjusted by `+ (val > 0)`.
/// Column pass: sums adjusted by `+ (val > 0)`, then normalized with >>3.
pub fn fwht4x4(input: &[i16; 16]) -> [i16; 16] {
    // Row transform (multiply by 4, with rounding adjustment)
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        let a = (input[i * 4] as i32 + input[i * 4 + 2] as i32) * 4;
        let d = (input[i * 4 + 1] as i32 + input[i * 4 + 3] as i32) * 4;
        let c = (input[i * 4 + 1] as i32 - input[i * 4 + 3] as i32) * 4;
        let b = (input[i * 4] as i32 - input[i * 4 + 2] as i32) * 4;

        let v0 = a + d;
        let v1 = b + c;
        let v2 = b - c;
        let v3 = a - d;
        tmp[i * 4] = v0 + i32::from(v0 > 0);
        tmp[i * 4 + 1] = v1 + i32::from(v1 > 0);
        tmp[i * 4 + 2] = v2 + i32::from(v2 > 0);
        tmp[i * 4 + 3] = v3 + i32::from(v3 > 0);
    }

    // Column transform with >>3 normalization
    let mut result = [0i16; 16];
    for i in 0..4 {
        let a = tmp[i] + tmp[i + 8];
        let d = tmp[i + 4] + tmp[i + 12];
        let c = tmp[i + 4] - tmp[i + 12];
        let b = tmp[i] - tmp[i + 8];

        let mut a2 = a + d;
        let mut b2 = b + c;
        let mut c2 = b - c;
        let mut d2 = a - d;
        a2 += i32::from(a2 > 0);
        b2 += i32::from(b2 > 0);
        c2 += i32::from(c2 > 0);
        d2 += i32::from(d2 > 0);

        result[i] = ((a2 + 3) >> 3) as i16;
        result[i + 4] = ((b2 + 3) >> 3) as i16;
        result[i + 8] = ((c2 + 3) >> 3) as i16;
        result[i + 12] = ((d2 + 3) >> 3) as i16;
    }
    result
}

/// Inverse 4x4 DCT (VP8 specification, Sections 14.3-14.4).
pub fn idct4x4(input: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];

    // Row pass.
    for i in 0..4 {
        let c0 = input[i * 4];
        let c1 = input[i * 4 + 1];
        let c2 = input[i * 4 + 2];
        let c3 = input[i * 4 + 3];

        let a1 = c0 + c2;
        let b1 = c0 - c2;
        let temp1 = (c1 * 35468 >> 16) - c3 - (c3 * 20091 >> 16);
        let temp2 = c1 + (c1 * 20091 >> 16) + (c3 * 35468 >> 16);

        tmp[i * 4] = a1 + temp2;
        tmp[i * 4 + 3] = a1 - temp2;
        tmp[i * 4 + 1] = b1 + temp1;
        tmp[i * 4 + 2] = b1 - temp1;
    }

    // Column pass (with +4 rounding bias, then >>3 normalization).
    let mut result = [0i32; 16];
    for i in 0..4 {
        let c0 = tmp[i];
        let c1 = tmp[i + 4];
        let c2 = tmp[i + 8];
        let c3 = tmp[i + 12];

        let a1 = c0 + c2;
        let b1 = c0 - c2;
        let temp1 = (c1 * 35468 >> 16) - c3 - (c3 * 20091 >> 16);
        let temp2 = c1 + (c1 * 20091 >> 16) + (c3 * 35468 >> 16);

        result[i] = (a1 + temp2 + 4) >> 3;
        result[i + 12] = (a1 - temp2 + 4) >> 3;
        result[i + 4] = (b1 + temp1 + 4) >> 3;
        result[i + 8] = (b1 - temp1 + 4) >> 3;
    }
    result
}

/// Inverse 4x4 Walsh-Hadamard transform for Y2 DC coefficients.
pub fn iwht4x4(input: &[i16; 16]) -> [i16; 16] {
    let mut tmp = [0i32; 16];

    for i in 0..4 {
        let a = input[i * 4] as i32;
        let b = input[i * 4 + 1] as i32;
        let c = input[i * 4 + 2] as i32;
        let d = input[i * 4 + 3] as i32;
        tmp[i * 4] = a + b + c + d;
        tmp[i * 4 + 1] = a + b - c - d;
        tmp[i * 4 + 2] = a - b - c + d;
        tmp[i * 4 + 3] = a - b + c - d;
    }

    let mut result = [0i16; 16];
    for i in 0..4 {
        let a = tmp[i];
        let b = tmp[i + 4];
        let c = tmp[i + 8];
        let d = tmp[i + 12];
        result[i] = ((a + b + c + d + 3) >> 3) as i16;
        result[i + 4] = ((a + b - c - d + 3) >> 3) as i16;
        result[i + 8] = ((a - b - c + d + 3) >> 3) as i16;
        result[i + 12] = ((a - b + c - d + 3) >> 3) as i16;
    }
    result
}
