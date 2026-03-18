/// Forward 4x4 DCT as specified in the VP8 specification.
///
/// Input is a 4x4 block in row-major order; output is the transformed coefficients.
pub fn fdct4x4(input: &[i16; 16]) -> [i16; 16] {
    // Row transform
    let mut tmp = [0i16; 16];
    for i in 0..4 {
        let a = input[i * 4] as i32;
        let b = input[i * 4 + 1] as i32;
        let c = input[i * 4 + 2] as i32;
        let d = input[i * 4 + 3] as i32;
        let t0 = a + d;
        let t1 = b + c;
        let t2 = b - c;
        let t3 = a - d;
        tmp[i * 4] = (t0 + t1) as i16;
        tmp[i * 4 + 1] = ((t2 * 2217 + t3 * 5352 + 14500) >> 12) as i16;
        tmp[i * 4 + 2] = (t0 - t1) as i16;
        tmp[i * 4 + 3] = ((t3 * 2217 - t2 * 5352 + 7500) >> 12) as i16;
    }

    // Column transform
    let mut result = [0i16; 16];
    for i in 0..4 {
        let a = tmp[i] as i32;
        let b = tmp[i + 4] as i32;
        let c = tmp[i + 8] as i32;
        let d = tmp[i + 12] as i32;
        let t0 = a + d;
        let t1 = b + c;
        let t2 = b - c;
        let t3 = a - d;
        result[i] = (t0 + t1) as i16;
        result[i + 4] = ((t2 * 2217 + t3 * 5352 + 14500) >> 12) as i16;
        result[i + 8] = (t0 - t1) as i16;
        result[i + 12] = ((t3 * 2217 - t2 * 5352 + 7500) >> 12) as i16;
    }
    result
}

/// Forward 4x4 Walsh-Hadamard transform for Y2 DC coefficients.
pub fn fwht4x4(input: &[i16; 16]) -> [i16; 16] {
    // Row transform
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

    // Column transform with normalization (>> 2)
    let mut result = [0i16; 16];
    for i in 0..4 {
        let a = tmp[i];
        let b = tmp[i + 4];
        let c = tmp[i + 8];
        let d = tmp[i + 12];
        result[i] = ((a + b + c + d) >> 2) as i16;
        result[i + 4] = ((a + b - c - d) >> 2) as i16;
        result[i + 8] = ((a - b - c + d) >> 2) as i16;
        result[i + 12] = ((a - b + c - d) >> 2) as i16;
    }
    result
}
