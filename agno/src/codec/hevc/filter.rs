/// HEVC deblocking filter and SAO (Sample Adaptive Offset).
///
/// Implements ITU-T H.265 sections 8.7.2 (deblocking) and 8.7.3 (SAO).
/// The deblocking filter operates on 8x8 block grid boundaries, filtering
/// vertical edges first, then horizontal edges. SAO is applied per-CTU
/// after deblocking to reduce banding and ringing artifacts.
use super::params::{Pps, Sps};
use super::picture::{Picture, SaoParams};

const BETA_TABLE: [i32; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
    20, 22, 24, 26, 28, 30, 32, 34, 36, 38, 40, 42, 44, 46, 48, 50, 52, 54, 56, 58, 60, 62, 64,
];

const TC_TABLE: [i32; 54] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 3,
    3, 3, 3, 4, 4, 4, 5, 5, 6, 6, 7, 8, 9, 10, 11, 13, 14, 16, 18, 20, 22, 24,
];

fn clip3(lo: i32, hi: i32, val: i32) -> i32 {
    val.clamp(lo, hi)
}

/// Derive boundary strength for all-intra content (H.265 8.7.2.5).
/// Returns 2 at CU boundaries (both sides intra), 0 within a CU.
/// If cu_depth is uninitialized (None), conservatively returns 2 to preserve
/// deblocking behavior for pictures without CU depth tracking.
fn derive_bs_intra(pic: &Picture, x: u32, y: u32, vertical: bool, ctb_log2: u32) -> i32 {
    if vertical {
        if x == 0 {
            return 0;
        }
        let dl = match pic.cu_depth_at(x - 1, y) {
            Some(d) => d as u32,
            None => return 2,
        };
        let dr = match pic.cu_depth_at(x, y) {
            Some(d) => d as u32,
            None => return 2,
        };
        let sl = 1u32 << (ctb_log2 - dl.min(ctb_log2));
        let sr = 1u32 << (ctb_log2 - dr.min(ctb_log2));
        if x.is_multiple_of(sl) || x.is_multiple_of(sr) {
            2
        } else {
            0
        }
    } else {
        if y == 0 {
            return 0;
        }
        let dt = match pic.cu_depth_at(x, y - 1) {
            Some(d) => d as u32,
            None => return 2,
        };
        let db = match pic.cu_depth_at(x, y) {
            Some(d) => d as u32,
            None => return 2,
        };
        let st = 1u32 << (ctb_log2 - dt.min(ctb_log2));
        let sb = 1u32 << (ctb_log2 - db.min(ctb_log2));
        if y.is_multiple_of(st) || y.is_multiple_of(sb) {
            2
        } else {
            0
        }
    }
}

/// Processes all 8x8 grid boundaries: vertical edges first, then horizontal.
/// For all-intra content (HEIC), boundary strength is 2 at CU/PU boundaries.
/// Per-CU QP is read from `pic.qp_y_at()` for each side of the edge (H.265 8.7.2.4).
pub fn deblock(pic: &mut Picture, sps: &Sps, pps: &Pps) {
    let bit_depth_y = sps.bit_depth_luma as i32;
    let bit_depth_c = sps.bit_depth_chroma as i32;
    let beta_offset = pps.pps_beta_offset_div2;
    let tc_offset = pps.pps_tc_offset_div2;
    let width = pic.width;
    let height = pic.height;

    // Vertical edges (filter columns)
    let ctb_log2 = sps.ctb_log2_size();
    let mut x = 8u32;
    while x < width {
        let mut y = 0u32;
        while y < height {
            let bs = derive_bs_intra(pic, x, y, true, ctb_log2);
            if bs == 0 {
                y += 4;
                continue;
            }

            // H.265 8.7.2.4: QP from each side of the vertical edge
            let qp_left = pic.qp_y_at(x - 1, y);
            let qp_right = pic.qp_y_at(x, y);
            let qp_avg = (qp_left + qp_right + 1) >> 1;

            // Luma
            let beta_idx = clip3(0, 51, qp_avg + (beta_offset << 1));
            let tc_idx = clip3(0, 53, qp_avg + 2 * (bs - 1) + (tc_offset << 1));
            let beta = BETA_TABLE[beta_idx as usize];
            let tc = TC_TABLE[tc_idx as usize];

            if tc > 0 {
                deblock_luma_edge_v(pic, x, y, beta, tc, bit_depth_y);
            }

            // Chroma (when Bs >= 2)
            let chroma_w = pic.chroma_width();
            let chroma_h = pic.chroma_height();
            if bs >= 2 && x / 2 < chroma_w && y / pic.sub_height_c() < chroma_h {
                let tc_c_idx = clip3(0, 53, qp_avg + 2 * (bs - 1) + (tc_offset << 1));
                let tc_c = TC_TABLE[tc_c_idx as usize];
                if tc_c > 0 {
                    deblock_chroma_edge_v(pic, x, y, tc_c, bit_depth_c);
                }
            }

            y += 4;
        }
        x += 8;
    }

    // Horizontal edges (filter rows)
    let mut y = 8u32;
    while y < height {
        let mut x = 0u32;
        while x < width {
            let bs = derive_bs_intra(pic, x, y, false, ctb_log2);
            if bs == 0 {
                x += 4;
                continue;
            }

            // H.265 8.7.2.4: QP from each side of the horizontal edge
            let qp_top = pic.qp_y_at(x, y - 1);
            let qp_bottom = pic.qp_y_at(x, y);
            let qp_avg = (qp_top + qp_bottom + 1) >> 1;

            let beta_idx = clip3(0, 51, qp_avg + (beta_offset << 1));
            let tc_idx = clip3(0, 53, qp_avg + 2 * (bs - 1) + (tc_offset << 1));
            let beta = BETA_TABLE[beta_idx as usize];
            let tc = TC_TABLE[tc_idx as usize];

            if tc > 0 {
                deblock_luma_edge_h(pic, x, y, beta, tc, bit_depth_y);
            }

            let chroma_w = pic.chroma_width();
            let chroma_h = pic.chroma_height();
            if bs >= 2 && x / 2 < chroma_w && y / pic.sub_height_c() < chroma_h {
                let tc_c_idx = clip3(0, 53, qp_avg + 2 * (bs - 1) + (tc_offset << 1));
                let tc_c = TC_TABLE[tc_c_idx as usize];
                if tc_c > 0 {
                    deblock_chroma_edge_h(pic, x, y, tc_c, bit_depth_c);
                }
            }

            x += 4;
        }
        y += 8;
    }
}

/// Read 8 samples (p3,p2,p1,p0,q0,q1,q2,q3) across a vertical edge for one line.
fn read_v_line(pic: &Picture, edge_x: u32, line_y: u32) -> [i32; 8] {
    [
        pic.y_at(edge_x - 4, line_y) as i32, // p3
        pic.y_at(edge_x - 3, line_y) as i32, // p2
        pic.y_at(edge_x - 2, line_y) as i32, // p1
        pic.y_at(edge_x - 1, line_y) as i32, // p0
        pic.y_at(edge_x, line_y) as i32,     // q0
        pic.y_at(edge_x + 1, line_y) as i32, // q1
        pic.y_at(edge_x + 2, line_y) as i32, // q2
        pic.y_at(edge_x + 3, line_y) as i32, // q3
    ]
}

/// Filter a 4-row vertical luma edge per H.265 8.7.2.5.
/// Lines 0 and 3 determine the filtering decision for all 4 lines.
fn deblock_luma_edge_v(pic: &mut Picture, edge_x: u32, y: u32, beta: i32, tc: i32, bit_depth: i32) {
    let max_val = (1 << bit_depth) - 1;

    if edge_x < 4 || edge_x + 3 >= pic.width {
        return;
    }
    if y + 3 >= pic.height {
        return;
    }

    let s0 = read_v_line(pic, edge_x, y);
    let s3 = read_v_line(pic, edge_x, y + 3);

    // dp/dq for lines 0 and 3 (H.265 8.7.2.5.3)
    let dp0 = (s0[1] - 2 * s0[2] + s0[3]).abs(); // |p2 - 2*p1 + p0|
    let dq0 = (s0[6] - 2 * s0[5] + s0[4]).abs(); // |q2 - 2*q1 + q0|
    let dp3 = (s3[1] - 2 * s3[2] + s3[3]).abs();
    let dq3 = (s3[6] - 2 * s3[5] + s3[4]).abs();

    let d = dp0 + dq0 + dp3 + dq3;
    if d >= beta {
        return;
    }

    // Strong filter decision using dSam0 and dSam3 (H.265 8.7.2.5.4)
    let tc5 = (5 * tc + 1) >> 1;
    let beta_2 = beta >> 2;
    let beta_3 = beta >> 3;

    let d_sam0 = 2 * (dp0 + dq0) < beta_2
        && (s0[0] - s0[3]).abs() + (s0[4] - s0[7]).abs() < beta_3
        && (s0[3] - s0[4]).abs() < tc5;
    let d_sam3 = 2 * (dp3 + dq3) < beta_2
        && (s3[0] - s3[3]).abs() + (s3[4] - s3[7]).abs() < beta_3
        && (s3[3] - s3[4]).abs() < tc5;

    let use_strong = d_sam0 && d_sam3;

    // Weak filter secondary thresholds (H.265 8.7.2.5.5)
    let dp = dp0 + dp3;
    let dq = dq0 + dq3;
    let side_thresh = (beta + (beta >> 1)) >> 3;
    let d_ep = dp < side_thresh;
    let d_eq = dq < side_thresh;

    // Apply to all 4 lines
    for dy in 0..4u32 {
        let py = y + dy;
        let s = read_v_line(pic, edge_x, py);
        let (p3, p2, p1, p0, q0, q1, q2, q3) = (s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]);

        if use_strong {
            let tc2 = 2 * tc;
            let p0f = clip3(
                p0 - tc2,
                p0 + tc2,
                (p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3,
            );
            let p1f = clip3(p1 - tc2, p1 + tc2, (p2 + p1 + p0 + q0 + 2) >> 2);
            let p2f = clip3(
                p2 - tc2,
                p2 + tc2,
                (2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3,
            );
            let q0f = clip3(
                q0 - tc2,
                q0 + tc2,
                (p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3,
            );
            let q1f = clip3(q1 - tc2, q1 + tc2, (q2 + q1 + q0 + p0 + 2) >> 2);
            let q2f = clip3(
                q2 - tc2,
                q2 + tc2,
                (2 * q3 + 3 * q2 + q1 + q0 + p0 + 4) >> 3,
            );
            pic.set_y(edge_x - 3, py, clip3(0, max_val, p2f) as i16);
            pic.set_y(edge_x - 2, py, clip3(0, max_val, p1f) as i16);
            pic.set_y(edge_x - 1, py, clip3(0, max_val, p0f) as i16);
            pic.set_y(edge_x, py, clip3(0, max_val, q0f) as i16);
            pic.set_y(edge_x + 1, py, clip3(0, max_val, q1f) as i16);
            pic.set_y(edge_x + 2, py, clip3(0, max_val, q2f) as i16);
        } else {
            let delta = (9 * (q0 - p0) - 3 * (q1 - p1) + 8) >> 4;
            if delta.abs() >= 10 * tc {
                continue;
            }
            let delta_c = clip3(-tc, tc, delta);
            pic.set_y(edge_x - 1, py, clip3(0, max_val, p0 + delta_c) as i16);
            pic.set_y(edge_x, py, clip3(0, max_val, q0 - delta_c) as i16);

            let tc_half = tc >> 1;
            if d_ep {
                let dp1 = clip3(-tc_half, tc_half, ((p2 + p0 + 1) >> 1) - p1 + delta_c / 2);
                pic.set_y(edge_x - 2, py, clip3(0, max_val, p1 + dp1) as i16);
            }
            if d_eq {
                let dq1 = clip3(-tc_half, tc_half, ((q2 + q0 + 1) >> 1) - q1 - delta_c / 2);
                pic.set_y(edge_x + 1, py, clip3(0, max_val, q1 + dq1) as i16);
            }
        }
    }
}

/// Read 8 samples (p3,p2,p1,p0,q0,q1,q2,q3) across a horizontal edge for one column.
fn read_h_col(pic: &Picture, col_x: u32, edge_y: u32) -> [i32; 8] {
    [
        pic.y_at(col_x, edge_y - 4) as i32, // p3
        pic.y_at(col_x, edge_y - 3) as i32, // p2
        pic.y_at(col_x, edge_y - 2) as i32, // p1
        pic.y_at(col_x, edge_y - 1) as i32, // p0
        pic.y_at(col_x, edge_y) as i32,     // q0
        pic.y_at(col_x, edge_y + 1) as i32, // q1
        pic.y_at(col_x, edge_y + 2) as i32, // q2
        pic.y_at(col_x, edge_y + 3) as i32, // q3
    ]
}

/// Filter a 4-column horizontal luma edge per H.265 8.7.2.5.
/// Columns 0 and 3 determine the filtering decision for all 4 columns.
fn deblock_luma_edge_h(pic: &mut Picture, x: u32, edge_y: u32, beta: i32, tc: i32, bit_depth: i32) {
    let max_val = (1 << bit_depth) - 1;

    if edge_y < 4 || edge_y + 3 >= pic.height {
        return;
    }
    if x + 3 >= pic.width {
        return;
    }

    let s0 = read_h_col(pic, x, edge_y);
    let s3 = read_h_col(pic, x + 3, edge_y);

    // dp/dq for columns 0 and 3 (H.265 8.7.2.5.3)
    let dp0 = (s0[1] - 2 * s0[2] + s0[3]).abs(); // |p2 - 2*p1 + p0|
    let dq0 = (s0[6] - 2 * s0[5] + s0[4]).abs(); // |q2 - 2*q1 + q0|
    let dp3 = (s3[1] - 2 * s3[2] + s3[3]).abs();
    let dq3 = (s3[6] - 2 * s3[5] + s3[4]).abs();

    let d = dp0 + dq0 + dp3 + dq3;
    if d >= beta {
        return;
    }

    // Strong filter decision using dSam0 and dSam3 (H.265 8.7.2.5.4)
    let tc5 = (5 * tc + 1) >> 1;
    let beta_2 = beta >> 2;
    let beta_3 = beta >> 3;

    let d_sam0 = 2 * (dp0 + dq0) < beta_2
        && (s0[0] - s0[3]).abs() + (s0[4] - s0[7]).abs() < beta_3
        && (s0[3] - s0[4]).abs() < tc5;
    let d_sam3 = 2 * (dp3 + dq3) < beta_2
        && (s3[0] - s3[3]).abs() + (s3[4] - s3[7]).abs() < beta_3
        && (s3[3] - s3[4]).abs() < tc5;

    let use_strong = d_sam0 && d_sam3;

    // Weak filter secondary thresholds (H.265 8.7.2.5.5)
    let dp = dp0 + dp3;
    let dq = dq0 + dq3;
    let side_thresh = (beta + (beta >> 1)) >> 3;
    let d_ep = dp < side_thresh;
    let d_eq = dq < side_thresh;

    // Apply to all 4 columns
    for dx in 0..4u32 {
        let px = x + dx;
        let s = read_h_col(pic, px, edge_y);
        let (p3, p2, p1, p0, q0, q1, q2, q3) = (s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]);

        if use_strong {
            let tc2 = 2 * tc;
            let p0f = clip3(
                p0 - tc2,
                p0 + tc2,
                (p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3,
            );
            let p1f = clip3(p1 - tc2, p1 + tc2, (p2 + p1 + p0 + q0 + 2) >> 2);
            let p2f = clip3(
                p2 - tc2,
                p2 + tc2,
                (2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3,
            );
            let q0f = clip3(
                q0 - tc2,
                q0 + tc2,
                (p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3,
            );
            let q1f = clip3(q1 - tc2, q1 + tc2, (q2 + q1 + q0 + p0 + 2) >> 2);
            let q2f = clip3(
                q2 - tc2,
                q2 + tc2,
                (2 * q3 + 3 * q2 + q1 + q0 + p0 + 4) >> 3,
            );
            pic.set_y(px, edge_y - 3, clip3(0, max_val, p2f) as i16);
            pic.set_y(px, edge_y - 2, clip3(0, max_val, p1f) as i16);
            pic.set_y(px, edge_y - 1, clip3(0, max_val, p0f) as i16);
            pic.set_y(px, edge_y, clip3(0, max_val, q0f) as i16);
            pic.set_y(px, edge_y + 1, clip3(0, max_val, q1f) as i16);
            pic.set_y(px, edge_y + 2, clip3(0, max_val, q2f) as i16);
        } else {
            let delta = (9 * (q0 - p0) - 3 * (q1 - p1) + 8) >> 4;
            if delta.abs() >= 10 * tc {
                continue;
            }
            let delta_c = clip3(-tc, tc, delta);
            pic.set_y(px, edge_y - 1, clip3(0, max_val, p0 + delta_c) as i16);
            pic.set_y(px, edge_y, clip3(0, max_val, q0 - delta_c) as i16);

            let tc_half = tc >> 1;
            if d_ep {
                let dp1 = clip3(-tc_half, tc_half, ((p2 + p0 + 1) >> 1) - p1 + delta_c / 2);
                pic.set_y(px, edge_y - 2, clip3(0, max_val, p1 + dp1) as i16);
            }
            if d_eq {
                let dq1 = clip3(-tc_half, tc_half, ((q2 + q0 + 1) >> 1) - q1 - delta_c / 2);
                pic.set_y(px, edge_y + 1, clip3(0, max_val, q1 + dq1) as i16);
            }
        }
    }
}

fn deblock_chroma_edge_v(pic: &mut Picture, edge_x: u32, y: u32, tc: i32, bit_depth: i32) {
    let max_val = (1 << bit_depth) - 1;
    let cx = edge_x / 2;
    let cy = y / pic.sub_height_c();
    let cw = pic.chroma_width();
    let ch = pic.chroma_height();
    // Number of chroma lines per luma 4-line block: 2 for 4:2:0, 4 for 4:2:2
    let num_lines = 4 / pic.sub_height_c();

    for dy in 0..num_lines {
        let c_y = cy + dy;
        if c_y >= ch || cx < 1 || cx >= cw {
            continue;
        }

        // Cb
        let p0 = pic.cb_at(cx - 1, c_y) as i32;
        let p1 = if cx >= 2 {
            pic.cb_at(cx - 2, c_y) as i32
        } else {
            p0
        };
        let q0 = pic.cb_at(cx, c_y) as i32;
        let q1 = if cx + 1 < cw {
            pic.cb_at(cx + 1, c_y) as i32
        } else {
            q0
        };
        let delta = clip3(-tc, tc, ((q0 - p0) * 4 + p1 - q1 + 4) >> 3);
        pic.set_cb(cx - 1, c_y, clip3(0, max_val, p0 + delta) as i16);
        pic.set_cb(cx, c_y, clip3(0, max_val, q0 - delta) as i16);

        // Cr
        let p0 = pic.cr_at(cx - 1, c_y) as i32;
        let p1 = if cx >= 2 {
            pic.cr_at(cx - 2, c_y) as i32
        } else {
            p0
        };
        let q0 = pic.cr_at(cx, c_y) as i32;
        let q1 = if cx + 1 < cw {
            pic.cr_at(cx + 1, c_y) as i32
        } else {
            q0
        };
        let delta = clip3(-tc, tc, ((q0 - p0) * 4 + p1 - q1 + 4) >> 3);
        pic.set_cr(cx - 1, c_y, clip3(0, max_val, p0 + delta) as i16);
        pic.set_cr(cx, c_y, clip3(0, max_val, q0 - delta) as i16);
    }
}

fn deblock_chroma_edge_h(pic: &mut Picture, x: u32, edge_y: u32, tc: i32, bit_depth: i32) {
    let max_val = (1 << bit_depth) - 1;
    let cx = x / 2;
    let cy = edge_y / pic.sub_height_c();
    let cw = pic.chroma_width();
    let ch = pic.chroma_height();

    for dx in 0..2u32 {
        let c_x = cx + dx;
        if c_x >= cw || cy < 1 || cy >= ch {
            continue;
        }

        // Cb
        let p0 = pic.cb_at(c_x, cy - 1) as i32;
        let p1 = if cy >= 2 {
            pic.cb_at(c_x, cy - 2) as i32
        } else {
            p0
        };
        let q0 = pic.cb_at(c_x, cy) as i32;
        let q1 = if cy + 1 < ch {
            pic.cb_at(c_x, cy + 1) as i32
        } else {
            q0
        };
        let delta = clip3(-tc, tc, ((q0 - p0) * 4 + p1 - q1 + 4) >> 3);
        pic.set_cb(c_x, cy - 1, clip3(0, max_val, p0 + delta) as i16);
        pic.set_cb(c_x, cy, clip3(0, max_val, q0 - delta) as i16);

        // Cr
        let p0 = pic.cr_at(c_x, cy - 1) as i32;
        let p1 = if cy >= 2 {
            pic.cr_at(c_x, cy - 2) as i32
        } else {
            p0
        };
        let q0 = pic.cr_at(c_x, cy) as i32;
        let q1 = if cy + 1 < ch {
            pic.cr_at(c_x, cy + 1) as i32
        } else {
            q0
        };
        let delta = clip3(-tc, tc, ((q0 - p0) * 4 + p1 - q1 + 4) >> 3);
        pic.set_cr(c_x, cy - 1, clip3(0, max_val, p0 + delta) as i16);
        pic.set_cr(c_x, cy, clip3(0, max_val, q0 - delta) as i16);
    }
}

/// Apply SAO (Sample Adaptive Offset) to the picture using per-CTU parameters.
pub fn apply_sao(pic: &mut Picture, sps: &Sps) {
    let ctb_size = sps.ctb_size();
    let w_ctbs = sps.pic_width_in_ctbs();
    let h_ctbs = sps.pic_height_in_ctbs();

    if pic.sao_params.is_empty() {
        return;
    }

    // Work on a snapshot to avoid aliasing issues during filtering
    let width = pic.width;
    let height = pic.height;
    let stride_y = pic.stride_y;
    let stride_c = pic.stride_c;
    let bit_depth = pic.bit_depth;

    let y_snap: Vec<i16> = pic.y_mut().to_vec();
    let cb_snap: Vec<i16> = pic.cb_mut().to_vec();
    let cr_snap: Vec<i16> = pic.cr_mut().to_vec();

    for ctu_y in 0..h_ctbs {
        for ctu_x in 0..w_ctbs {
            let ctu_idx = (ctu_y * w_ctbs + ctu_x) as usize;
            if ctu_idx >= pic.sao_params.len() {
                continue;
            }
            let ctu_sao = pic.sao_params[ctu_idx].clone();

            let x0 = ctu_x * ctb_size;
            let y0 = ctu_y * ctb_size;

            // Luma
            apply_sao_component(
                pic.y_mut(),
                &y_snap,
                stride_y,
                width,
                height,
                x0,
                y0,
                ctb_size,
                ctb_size,
                &ctu_sao.y,
                bit_depth,
            );

            // Chroma
            let cw = pic.chroma_width();
            let ch = pic.chroma_height();
            let cx0 = x0 / pic.sub_width_c();
            let cy0 = y0 / pic.sub_height_c();
            let c_ctb_w = ctb_size / pic.sub_width_c();
            let c_ctb_h = ctb_size / pic.sub_height_c();

            apply_sao_component(
                pic.cb_mut(),
                &cb_snap,
                stride_c,
                cw,
                ch,
                cx0,
                cy0,
                c_ctb_w,
                c_ctb_h,
                &ctu_sao.cb,
                bit_depth,
            );
            apply_sao_component(
                pic.cr_mut(),
                &cr_snap,
                stride_c,
                cw,
                ch,
                cx0,
                cy0,
                c_ctb_w,
                c_ctb_h,
                &ctu_sao.cr,
                bit_depth,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_sao_component(
    dst: &mut [i16],
    src: &[i16],
    stride: u32,
    pic_w: u32,
    pic_h: u32,
    x0: u32,
    y0: u32,
    ctb_w: u32,
    ctb_h: u32,
    sao: &SaoParams,
    bit_depth: u8,
) {
    if sao.sao_type_idx == 0 {
        return;
    }
    let max_val = (1i32 << bit_depth) - 1;

    let x_end = (x0 + ctb_w).min(pic_w);
    let y_end = (y0 + ctb_h).min(pic_h);

    if sao.sao_type_idx == 1 {
        // Band offset
        let shift = bit_depth.saturating_sub(5);
        for y in y0..y_end {
            for x in x0..x_end {
                let idx = (y * stride + x) as usize;
                let sample = src[idx] as i32;
                let band = (sample >> shift) & 31;
                let band_start = sao.sao_band_position as i32;
                let band_off = band - band_start;
                if (0..4).contains(&band_off) {
                    let offset = sao.sao_offset[band_off as usize] as i32;
                    dst[idx] = clip3(0, max_val, sample + offset) as i16;
                }
            }
        }
    } else if sao.sao_type_idx == 2 {
        // Edge offset
        let (dx0, dy0, dx1, dy1): (i32, i32, i32, i32) = match sao.sao_eo_class {
            0 => (-1, 0, 1, 0),  // horizontal
            1 => (0, -1, 0, 1),  // vertical
            2 => (-1, -1, 1, 1), // 135-degree diagonal
            _ => (1, -1, -1, 1), // 45-degree diagonal
        };

        // Edge offset categories map to offsets:
        // cat 1 (valley): offset[0], cat 2 (valley-weak): offset[1]
        // cat 3 (peak-weak): offset[2], cat 4 (peak): offset[3]
        let offsets = [
            sao.sao_offset[0] as i32, // cat 1
            sao.sao_offset[1] as i32, // cat 2
            0,                        // cat 0 (flat)
            sao.sao_offset[2] as i32, // cat 3
            sao.sao_offset[3] as i32, // cat 4
        ];

        for y in y0..y_end {
            for x in x0..x_end {
                let nx0 = x as i32 + dx0;
                let ny0 = y as i32 + dy0;
                let nx1 = x as i32 + dx1;
                let ny1 = y as i32 + dy1;

                if nx0 < 0
                    || nx0 >= pic_w as i32
                    || ny0 < 0
                    || ny0 >= pic_h as i32
                    || nx1 < 0
                    || nx1 >= pic_w as i32
                    || ny1 < 0
                    || ny1 >= pic_h as i32
                {
                    continue;
                }

                let idx = (y * stride + x) as usize;
                let c = src[idx] as i32;
                let a = src[(ny0 as u32 * stride + nx0 as u32) as usize] as i32;
                let b = src[(ny1 as u32 * stride + nx1 as u32) as usize] as i32;

                let sign_a = (c - a).signum();
                let sign_b = (c - b).signum();
                let cat = (sign_a + sign_b + 2) as usize;
                let offset = offsets[cat];
                if offset != 0 {
                    dst[idx] = clip3(0, max_val, c + offset) as i16;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::params::ProfileTierLevel;
    use super::super::picture::CtuSaoParams;
    use super::*;

    fn test_sps(w: u32, h: u32) -> Sps {
        Sps {
            _sps_video_parameter_set_id: 0,
            _sps_max_sub_layers_minus1: 0,
            _sps_temporal_id_nesting_flag: true,
            _profile_tier_level: ProfileTierLevel::default(),
            _sps_seq_parameter_set_id: 0,
            _chroma_format_idc: 1,
            _separate_colour_plane_flag: false,
            pic_width_in_luma_samples: w,
            pic_height_in_luma_samples: h,
            _conformance_window_flag: false,
            _conf_win_left_offset: 0,
            _conf_win_right_offset: 0,
            _conf_win_top_offset: 0,
            _conf_win_bottom_offset: 0,
            bit_depth_luma_minus8: 0,
            _bit_depth_chroma_minus8: 0,
            bit_depth_luma: 8,
            bit_depth_chroma: 8,
            _log2_max_pic_order_cnt_lsb_minus4: 4,
            _sps_sub_layer_ordering_info_present_flag: false,
            log2_min_luma_coding_block_size_minus3: 0,
            log2_diff_max_min_luma_coding_block_size: 3,
            log2_min_luma_transform_block_size_minus2: 0,
            log2_diff_max_min_luma_transform_block_size: 3,
            _max_transform_hierarchy_depth_inter: 1,
            max_transform_hierarchy_depth_intra: 1,
            scaling_list_enabled_flag: false,
            scaling_list: None,
            _amp_enabled_flag: true,
            sample_adaptive_offset_enabled_flag: true,
            _pcm_enabled_flag: false,
            _pcm_sample_bit_depth_luma_minus1: 0,
            _pcm_sample_bit_depth_chroma_minus1: 0,
            _log2_min_pcm_luma_coding_block_size_minus3: 0,
            _log2_diff_max_min_pcm_luma_coding_block_size: 0,
            _pcm_loop_filter_disabled_flag: false,
            _num_short_term_ref_pic_sets: 0,
            _short_term_ref_pic_sets: vec![],
            _long_term_ref_pics_present_flag: false,
            _num_long_term_ref_pics_sps: 0,
            _sps_temporal_mvp_enabled_flag: true,
            strong_intra_smoothing_enabled_flag: true,
            _vui_parameters_present_flag: false,
            matrix_coefficients: 0,
            persistent_rice_adaptation_enabled_flag: false,
            cabac_bypass_alignment_enabled_flag: false,
            _transform_skip_rotation_enabled_flag: false,
            _transform_skip_context_enabled_flag: false,
            _implicit_rdpcm_enabled_flag: false,
            _explicit_rdpcm_enabled_flag: false,
            _extended_precision_processing_flag: false,
            _intra_smoothing_disabled_flag: false,
            _high_precision_offsets_enabled_flag: false,
        }
    }

    fn test_sps_10bit(w: u32, h: u32) -> Sps {
        let mut sps = test_sps(w, h);
        sps.bit_depth_luma_minus8 = 2;
        sps._bit_depth_chroma_minus8 = 2;
        sps.bit_depth_luma = 10;
        sps.bit_depth_chroma = 10;
        sps
    }

    fn test_pps() -> Pps {
        Pps {
            _pps_pic_parameter_set_id: 0,
            _pps_seq_parameter_set_id: 0,
            _dependent_slice_segments_enabled_flag: false,
            output_flag_present_flag: false,
            num_extra_slice_header_bits: 0,
            sign_data_hiding_enabled_flag: false,
            _cabac_init_present_flag: false,
            _num_ref_idx_l0_default_active_minus1: 0,
            _num_ref_idx_l1_default_active_minus1: 0,
            init_qp_minus26: 0,
            _constrained_intra_pred_flag: false,
            transform_skip_enabled_flag: false,
            _cu_qp_delta_enabled_flag: false,
            _diff_cu_qp_delta_depth: 0,
            pps_cb_qp_offset: 0,
            pps_cr_qp_offset: 0,
            pps_slice_chroma_qp_offsets_present_flag: false,
            _weighted_pred_flag: false,
            _weighted_bipred_flag: false,
            _transquant_bypass_enabled_flag: false,
            _tiles_enabled_flag: false,
            _entropy_coding_sync_enabled_flag: false,
            _num_tile_columns_minus1: 0,
            _num_tile_rows_minus1: 0,
            _uniform_spacing_flag: true,
            _loop_filter_across_tiles_enabled_flag: true,
            _loop_filter_across_slices_enabled_flag: true,
            _deblocking_filter_control_present_flag: false,
            deblocking_filter_override_enabled_flag: false,
            pps_deblocking_filter_disabled_flag: false,
            pps_beta_offset_div2: 0,
            pps_tc_offset_div2: 0,
            scaling_list: None,
            _lists_modification_present_flag: false,
            _log2_parallel_merge_level_minus2: 0,
            _slice_segment_header_extension_present_flag: false,
        }
    }

    fn flat_picture(w: u32, h: u32, val: i16) -> Picture {
        flat_picture_with_qp(w, h, val, 30)
    }

    fn flat_picture_with_qp(w: u32, h: u32, val: i16, qp: i32) -> Picture {
        let mut pic = Picture::new(w, h, 8, 1);
        // min_cb_log2=3 matches test_sps()
        pic.init_metadata(3, qp);
        for s in pic.y_mut().iter_mut() {
            *s = val;
        }
        for s in pic.cb_mut().iter_mut() {
            *s = val;
        }
        for s in pic.cr_mut().iter_mut() {
            *s = val;
        }
        pic
    }

    #[test]
    fn clip3_clamps_below() {
        assert_eq!(clip3(10, 20, 5), 10);
    }

    #[test]
    fn clip3_clamps_above() {
        assert_eq!(clip3(10, 20, 25), 20);
    }

    #[test]
    fn clip3_within_range() {
        assert_eq!(clip3(10, 20, 15), 15);
    }

    #[test]
    fn clip3_at_boundaries() {
        assert_eq!(clip3(10, 20, 10), 10);
        assert_eq!(clip3(10, 20, 20), 20);
    }

    #[test]
    fn deblock_flat_picture_unchanged() {
        let sps = test_sps(16, 16);
        let pps = test_pps();
        let mut pic = flat_picture_with_qp(16, 16, 128, 30);
        let original: Vec<i16> = pic.y_mut().to_vec();
        deblock(&mut pic, &sps, &pps);
        assert_eq!(pic.y_mut(), original.as_slice());
    }

    #[test]
    fn deblock_small_picture_no_panic() {
        let sps = test_sps(8, 8);
        let pps = test_pps();
        let mut pic = flat_picture_with_qp(8, 8, 100, 25);
        deblock(&mut pic, &sps, &pps);
    }

    #[test]
    fn deblock_modifies_vertical_step_edge() {
        let sps = test_sps(16, 16);
        let pps = test_pps();
        let mut pic = Picture::new(16, 16, 8, 1);
        pic.init_metadata(3, 40);
        for y in 0..16u32 {
            for x in 0..16u32 {
                pic.set_y(x, y, if x < 8 { 50 } else { 200 });
            }
        }
        for s in pic.cb_mut().iter_mut() {
            *s = 128;
        }
        for s in pic.cr_mut().iter_mut() {
            *s = 128;
        }

        deblock(&mut pic, &sps, &pps);

        let p0 = pic.y_at(7, 0);
        let q0 = pic.y_at(8, 0);
        assert!(p0 != 50 || q0 != 200, "deblock should soften boundary");
    }

    #[test]
    fn deblock_modifies_horizontal_step_edge() {
        let sps = test_sps(16, 16);
        let pps = test_pps();
        let mut pic = Picture::new(16, 16, 8, 1);
        pic.init_metadata(3, 40);
        for y in 0..16u32 {
            for x in 0..16u32 {
                pic.set_y(x, y, if y < 8 { 50 } else { 200 });
            }
        }
        for s in pic.cb_mut().iter_mut() {
            *s = 128;
        }
        for s in pic.cr_mut().iter_mut() {
            *s = 128;
        }

        deblock(&mut pic, &sps, &pps);

        let p0 = pic.y_at(0, 7);
        let q0 = pic.y_at(0, 8);
        assert!(
            p0 != 50 || q0 != 200,
            "deblock should soften horizontal boundary"
        );
    }

    #[test]
    fn deblock_strong_filter_modifies_p2() {
        // Use a step edge (flat on each side) so that dSam conditions are met:
        // |p3-p0| + |q0-q3| must be < beta>>3, and |p0-q0| < (5*tc+1)>>1
        let sps = test_sps(16, 16);
        let pps = test_pps();
        let mut pic = Picture::new(16, 16, 8, 1);
        pic.init_metadata(3, 40);
        for y in 0..16u32 {
            for x in 0..16u32 {
                pic.set_y(x, y, if x < 8 { 120 } else { 130 });
            }
        }
        for s in pic.cb_mut().iter_mut() {
            *s = 128;
        }
        for s in pic.cr_mut().iter_mut() {
            *s = 128;
        }

        let before_p2 = pic.y_at(5, 0);
        deblock(&mut pic, &sps, &pps);
        let after_p2 = pic.y_at(5, 0);
        assert_ne!(before_p2, after_p2, "strong filter should modify p2");
    }

    #[test]
    fn deblock_10bit_stays_in_range() {
        let sps = test_sps_10bit(16, 16);
        let pps = test_pps();
        let mut pic = Picture::new(16, 16, 10, 1);
        pic.init_metadata(3, 35);
        for y in 0..16u32 {
            for x in 0..16u32 {
                pic.set_y(x, y, if x < 8 { 200 } else { 800 });
            }
        }
        for s in pic.cb_mut().iter_mut() {
            *s = 512;
        }
        for s in pic.cr_mut().iter_mut() {
            *s = 512;
        }

        deblock(&mut pic, &sps, &pps);

        for y in 0..16u32 {
            for x in 0..16u32 {
                let v = pic.y_at(x, y);
                assert!(v >= 0 && v <= 1023, "({x},{y})={v} out of 10-bit range");
            }
        }
    }

    #[test]
    fn deblock_chroma_step_edge() {
        let sps = test_sps(16, 16);
        let pps = test_pps();
        let mut pic = Picture::new(16, 16, 8, 1);
        pic.init_metadata(3, 30);
        for s in pic.y_mut().iter_mut() {
            *s = 128;
        }
        let c_w = (16 + 1) / 2;
        for cy in 0..((16 + 1) / 2) as u32 {
            for cx in 0..c_w as u32 {
                let val = if cx < 4 { 50i16 } else { 200i16 };
                pic.set_cb(cx, cy, val);
                pic.set_cr(cx, cy, val);
            }
        }

        deblock(&mut pic, &sps, &pps);

        let cb_p0 = pic.cb_at(3, 0);
        let cb_q0 = pic.cb_at(4, 0);
        assert!(
            cb_p0 != 50 || cb_q0 != 200,
            "chroma deblock should modify boundary"
        );
    }

    #[test]
    fn sao_type_none_unchanged() {
        let sps = test_sps(64, 64);
        let mut pic = flat_picture(64, 64, 128);
        let num_ctus = (sps.pic_width_in_ctbs() * sps.pic_height_in_ctbs()) as usize;
        pic.sao_params = vec![CtuSaoParams::default(); num_ctus];

        let original: Vec<i16> = pic.y_mut().to_vec();
        apply_sao(&mut pic, &sps);
        assert_eq!(pic.y_mut(), original.as_slice());
    }

    #[test]
    fn sao_band_offset_applies() {
        let sps = test_sps(64, 64);
        let mut pic = flat_picture(64, 64, 128);
        pic.sao_params = vec![CtuSaoParams {
            y: SaoParams {
                sao_type_idx: 1,
                sao_offset: [2, 0, 0, 0],
                sao_band_position: 16,
                sao_eo_class: 0,
            },
            cb: SaoParams::default(),
            cr: SaoParams::default(),
        }];

        apply_sao(&mut pic, &sps);
        assert!(pic.y_mut().iter().all(|&v| v == 130));
        assert!(pic.cb_mut().iter().all(|&v| v == 128));
    }

    #[test]
    fn sao_band_offset_clips_to_max() {
        let sps = test_sps(64, 64);
        let mut pic = flat_picture(64, 64, 254);
        pic.sao_params = vec![CtuSaoParams {
            y: SaoParams {
                sao_type_idx: 1,
                sao_offset: [10, 0, 0, 0],
                sao_band_position: 31,
                sao_eo_class: 0,
            },
            cb: SaoParams::default(),
            cr: SaoParams::default(),
        }];

        apply_sao(&mut pic, &sps);
        assert!(pic.y_mut().iter().all(|&v| v == 255));
    }

    #[test]
    fn sao_band_offset_no_change_outside_bands() {
        let sps = test_sps(64, 64);
        let mut pic = flat_picture(64, 64, 0);
        pic.sao_params = vec![CtuSaoParams {
            y: SaoParams {
                sao_type_idx: 1,
                sao_offset: [5, 5, 5, 5],
                sao_band_position: 16,
                sao_eo_class: 0,
            },
            cb: SaoParams::default(),
            cr: SaoParams::default(),
        }];

        apply_sao(&mut pic, &sps);
        assert!(pic.y_mut().iter().all(|&v| v == 0));
    }

    #[test]
    fn sao_edge_offset_horizontal_valley() {
        let sps = test_sps(64, 64);
        let mut pic = flat_picture(64, 64, 80);
        pic.set_y(2, 0, 50);
        pic.sao_params = vec![CtuSaoParams {
            y: SaoParams {
                sao_type_idx: 2,
                sao_offset: [5, 0, 0, 0],
                sao_band_position: 0,
                sao_eo_class: 0,
            },
            cb: SaoParams::default(),
            cr: SaoParams::default(),
        }];

        apply_sao(&mut pic, &sps);
        assert_eq!(pic.y_at(2, 0), 55);
    }

    #[test]
    fn sao_edge_offset_vertical_peak() {
        let sps = test_sps(64, 64);
        let mut pic = flat_picture(64, 64, 80);
        pic.set_y(0, 2, 200);
        pic.sao_params = vec![CtuSaoParams {
            y: SaoParams {
                sao_type_idx: 2,
                sao_offset: [0, 0, 0, 4],
                sao_band_position: 0,
                sao_eo_class: 1,
            },
            cb: SaoParams::default(),
            cr: SaoParams::default(),
        }];

        apply_sao(&mut pic, &sps);
        assert_eq!(pic.y_at(0, 2), 204);
    }

    #[test]
    fn sao_edge_offset_diag_135() {
        let sps = test_sps(64, 64);
        let mut pic = flat_picture(64, 64, 80);
        pic.set_y(1, 1, 50);
        pic.sao_params = vec![CtuSaoParams {
            y: SaoParams {
                sao_type_idx: 2,
                sao_offset: [10, 0, 0, 0],
                sao_band_position: 0,
                sao_eo_class: 2,
            },
            cb: SaoParams::default(),
            cr: SaoParams::default(),
        }];

        apply_sao(&mut pic, &sps);
        assert_eq!(pic.y_at(1, 1), 60);
    }

    #[test]
    fn sao_edge_offset_diag_45() {
        let sps = test_sps(64, 64);
        let mut pic = flat_picture(64, 64, 80);
        pic.set_y(1, 1, 50);
        pic.sao_params = vec![CtuSaoParams {
            y: SaoParams {
                sao_type_idx: 2,
                sao_offset: [10, 0, 0, 0],
                sao_band_position: 0,
                sao_eo_class: 3,
            },
            cb: SaoParams::default(),
            cr: SaoParams::default(),
        }];

        apply_sao(&mut pic, &sps);
        assert_eq!(pic.y_at(1, 1), 60);
    }

    #[test]
    fn sao_edge_offset_skips_picture_boundary() {
        let sps = test_sps(64, 64);
        let mut pic = flat_picture(64, 64, 50);
        pic.sao_params = vec![CtuSaoParams {
            y: SaoParams {
                sao_type_idx: 2,
                sao_offset: [10, 10, 10, 10],
                sao_band_position: 0,
                sao_eo_class: 0,
            },
            cb: SaoParams::default(),
            cr: SaoParams::default(),
        }];

        apply_sao(&mut pic, &sps);
        assert_eq!(pic.y_at(0, 0), 50);
    }

    #[test]
    fn sao_edge_flat_region_unchanged() {
        let sps = test_sps(64, 64);
        let mut pic = flat_picture(64, 64, 100);
        pic.sao_params = vec![CtuSaoParams {
            y: SaoParams {
                sao_type_idx: 2,
                sao_offset: [5, 3, -3, -5],
                sao_band_position: 0,
                sao_eo_class: 0,
            },
            cb: SaoParams::default(),
            cr: SaoParams::default(),
        }];

        let original: Vec<i16> = pic.y_mut().to_vec();
        apply_sao(&mut pic, &sps);
        assert_eq!(pic.y_mut(), original.as_slice());
    }

    #[test]
    fn sao_skips_missing_ctu_params() {
        let sps = test_sps(128, 128);
        let mut pic = flat_picture(128, 128, 128);
        pic.sao_params = vec![CtuSaoParams {
            y: SaoParams {
                sao_type_idx: 1,
                sao_offset: [1, 0, 0, 0],
                sao_band_position: 16,
                sao_eo_class: 0,
            },
            cb: SaoParams::default(),
            cr: SaoParams::default(),
        }];
        apply_sao(&mut pic, &sps);
    }

    #[test]
    fn sao_empty_params_noop() {
        let sps = test_sps(64, 64);
        let mut pic = flat_picture(64, 64, 128);
        let original: Vec<i16> = pic.y_mut().to_vec();
        apply_sao(&mut pic, &sps);
        assert_eq!(pic.y_mut(), original.as_slice());
    }

    #[test]
    fn sao_10bit_band_offset() {
        let sps = test_sps_10bit(64, 64);
        let mut pic = Picture::new(64, 64, 10, 1);
        for s in pic.y_mut().iter_mut() {
            *s = 512;
        }
        for s in pic.cb_mut().iter_mut() {
            *s = 512;
        }
        for s in pic.cr_mut().iter_mut() {
            *s = 512;
        }
        pic.sao_params = vec![CtuSaoParams {
            y: SaoParams {
                sao_type_idx: 1,
                sao_offset: [5, 0, 0, 0],
                sao_band_position: 16,
                sao_eo_class: 0,
            },
            cb: SaoParams::default(),
            cr: SaoParams::default(),
        }];

        apply_sao(&mut pic, &sps);
        assert!(pic.y_mut().iter().all(|&v| v == 517));
    }

    #[test]
    fn sao_chroma_band_offset() {
        let sps = test_sps(64, 64);
        let mut pic = flat_picture(64, 64, 128);
        pic.sao_params = vec![CtuSaoParams {
            y: SaoParams::default(),
            cb: SaoParams {
                sao_type_idx: 1,
                sao_offset: [3, 0, 0, 0],
                sao_band_position: 16,
                sao_eo_class: 0,
            },
            cr: SaoParams {
                sao_type_idx: 1,
                sao_offset: [-2, 0, 0, 0],
                sao_band_position: 16,
                sao_eo_class: 0,
            },
        }];

        apply_sao(&mut pic, &sps);
        assert!(pic.y_mut().iter().all(|&v| v == 128));
        assert!(pic.cb_mut().iter().all(|&v| v == 131));
        assert!(pic.cr_mut().iter().all(|&v| v == 126));
    }
}
