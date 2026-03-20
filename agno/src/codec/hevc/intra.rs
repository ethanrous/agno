/// HEVC intra prediction (ITU-T H.265 / ISO 23008-2, section 8.4.4).
///
/// Implements all 35 luma intra prediction modes:
///   Mode  0 — Planar (bilinear interpolation)
///   Mode  1 — DC (flat average)
///   Modes 2..34 — Angular (33 directional angles)
///
/// Reference sample construction follows section 8.4.4.2, with substitution
/// for unavailable neighbors and optional strong intra smoothing for 32x32
/// blocks.

use super::picture::Picture;

// ---------------------------------------------------------------------------
// Component identifier
// ---------------------------------------------------------------------------

/// YCbCr color component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    Y,
    Cb,
    Cr,
}

// ---------------------------------------------------------------------------
// Angle tables (H.265 Table 8-4 and Table 8-5)
// ---------------------------------------------------------------------------

/// `intraPredAngle` for modes 2..34 (index 0 = mode 2, index 32 = mode 34).
const INTRA_PRED_ANGLE: [i32; 33] = [
    32, 26, 21, 17, 13, 9, 5, 2, 0, -2, -5, -9, -13, -17, -21, -26, -32,
    -26, -21, -17, -13, -9, -5, -2, 0, 2, 5, 9, 13, 17, 21, 26, 32,
];

/// `invAngle` for negative `intraPredAngle` values.
/// Indexed by the magnitude bucket: |angle| ∈ {2, 5, 9, 13, 17, 21, 26}.
const INV_ANGLE: [i32; 7] = [256, 315, 390, 512, 630, 819, 1024];

/// Map a negative `intraPredAngle` to the corresponding `invAngle` entry.
fn inv_angle_for(angle: i32) -> i32 {
    match angle.unsigned_abs() {
        2 => INV_ANGLE[0],
        5 => INV_ANGLE[1],
        9 => INV_ANGLE[2],
        13 => INV_ANGLE[3],
        17 => INV_ANGLE[4],
        21 => INV_ANGLE[5],
        26 => INV_ANGLE[6],
        _ => INV_ANGLE[0], // fallback, should not occur for valid modes
    }
}

// ---------------------------------------------------------------------------
// Reference sample construction (H.265 8.4.4.2)
// ---------------------------------------------------------------------------

/// Reference samples surrounding a transform block.
///
/// Layout (total 4*nTbS + 1 entries):
/// ```text
///   top_left  = p[-1][-1]
///   top[i]    = p[i][-1]   for i in 0 .. 2*nTbS - 1   (top row + top-right)
///   left[j]   = p[-1][j]   for j in 0 .. 2*nTbS - 1   (left column + bottom-left)
/// ```
struct RefSamples {
    top_left: i16,
    top: Vec<i16>,  // length = 2 * nTbS
    left: Vec<i16>, // length = 2 * nTbS
}

/// Read a sample from the correct plane of the picture.
///
/// Returns `None` when `(sx, sy)` is outside the plane bounds or when the
/// sample has not yet been decoded (H.265 8.4.4.2.2 availability check).
/// Uses the CU depth map to determine whether a sample position has been
/// reconstructed: positions with depth 0xFF have not been decoded yet.
fn read_sample(pic: &Picture, comp: Component, sx: i32, sy: i32) -> Option<i16> {
    if sx < 0 || sy < 0 {
        return None;
    }
    let (pw, ph, stride) = plane_dims(pic, comp);
    let ux = sx as u32;
    let uy = sy as u32;
    if ux >= pw || uy >= ph {
        return None;
    }

    // Check whether this position has been decoded using the CU depth map.
    // The depth map operates in luma coordinates at MinCB granularity.
    // Only perform this check when metadata has been initialized (during
    // actual HEVC slice decoding). When metadata is absent (unit tests,
    // standalone prediction), all in-bounds samples are considered available.
    if pic.has_metadata() {
        let (luma_x, luma_y) = match comp {
            Component::Y => (ux, uy),
            Component::Cb | Component::Cr => (ux * 2, uy * 2),
        };
        if pic.cu_depth_at(luma_x, luma_y).is_none() {
            return None;
        }
    }

    let val = match comp {
        Component::Y => pic.y_at(ux, uy),
        Component::Cb => pic.cb_at(ux, uy),
        Component::Cr => pic.cr_at(ux, uy),
    };
    let _ = stride;
    Some(val)
}

/// Return (width, height, stride) of the plane for `comp`.
fn plane_dims(pic: &Picture, comp: Component) -> (u32, u32, u32) {
    match comp {
        Component::Y => (pic.width, pic.height, pic.stride_y),
        Component::Cb | Component::Cr => {
            let cw = (pic.width + 1) / 2;
            let ch = (pic.height + 1) / 2;
            (cw, ch, pic.stride_c)
        }
    }
}

/// Build the reference sample array for a block at `(bx, by)` of size `n_tb_s`.
///
/// Coordinates `(bx, by)` are in component-plane samples (already adjusted for
/// chroma subsampling by the caller).
fn build_reference_samples(
    pic: &Picture,
    bx: u32,
    by: u32,
    n_tb_s: u32,
    comp: Component,
) -> RefSamples {
    let n = n_tb_s as i32;
    let total_side = 2 * n; // number of top or left reference samples

    // ------------------------------------------------------------------
    // 1. Gather raw samples, tracking availability.
    // ------------------------------------------------------------------
    let mut raw_top: Vec<Option<i16>> = Vec::with_capacity(total_side as usize);
    let mut raw_left: Vec<Option<i16>> = Vec::with_capacity(total_side as usize);

    let bx_i = bx as i32;
    let by_i = by as i32;

    // p[-1][-1]
    let mut raw_top_left = read_sample(pic, comp, bx_i - 1, by_i - 1);

    // p[x'][-1] for x' = 0 .. 2*nTbS - 1
    for i in 0..total_side {
        raw_top.push(read_sample(pic, comp, bx_i + i, by_i - 1));
    }

    // p[-1][y'] for y' = 0 .. 2*nTbS - 1
    for j in 0..total_side {
        raw_left.push(read_sample(pic, comp, bx_i - 1, by_i + j));
    }

    // ------------------------------------------------------------------
    // 2. Substitution: replicate nearest available sample (H.265 8.4.4.2.2).
    //
    // Scan order: bottom-left → left column upward → top-left → top row →
    // top-right. The first available sample is used to fill all preceding
    // unavailable positions; subsequent unavailable samples copy the last
    // available value seen so far.
    // ------------------------------------------------------------------
    let first_available = find_first_available(&raw_left, raw_top_left, &raw_top);

    // If nothing is available at all, use the mid-range value for the bit depth.
    let fallback = first_available.unwrap_or(1 << (pic.bit_depth.saturating_sub(1)));

    // Fill left (bottom to top)
    let mut last = fallback;
    for j in (0..total_side as usize).rev() {
        match raw_left[j] {
            Some(v) => last = v,
            None => raw_left[j] = Some(last),
        }
    }

    // Fill top-left
    if raw_top_left.is_none() {
        raw_top_left = Some(last);
    }

    // Fill top (left to right)
    last = raw_top_left.unwrap_or(fallback);
    for i in 0..total_side as usize {
        match raw_top[i] {
            Some(v) => last = v,
            None => raw_top[i] = Some(last),
        }
    }

    // Unwrap all — every entry is now Some.
    let top_left = raw_top_left.unwrap_or(fallback);
    let mut top: Vec<i16> = raw_top.into_iter().map(|v| v.unwrap_or(fallback)).collect();
    let mut left: Vec<i16> = raw_left.into_iter().map(|v| v.unwrap_or(fallback)).collect();

    RefSamples {
        top_left,
        top,
        left,
    }
}

/// Scan through left (bottom to top), top-left, top (left to right) to find
/// the first available sample value. Returns `None` if none are available.
fn find_first_available(
    raw_left: &[Option<i16>],
    raw_top_left: Option<i16>,
    raw_top: &[Option<i16>],
) -> Option<i16> {
    // Bottom-left to top of left column
    for j in (0..raw_left.len()).rev() {
        if let Some(v) = raw_left[j] {
            return Some(v);
        }
    }
    if let Some(v) = raw_top_left {
        return Some(v);
    }
    // Left to right along top
    for i in 0..raw_top.len() {
        if let Some(v) = raw_top[i] {
            return Some(v);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Reference sample filtering (H.265 8.4.4.2.3)
// ---------------------------------------------------------------------------

/// Threshold table for deciding whether to filter reference samples.
/// Indexed by log2(nTbS) - 3: nTbS=8 → index 0, nTbS=16 → 1, nTbS=32 → 2.
const INTRA_HOR_VER_DIST_THRESH: [u32; 3] = [7, 1, 0];

/// Apply reference sample filtering based on prediction mode (H.265 8.4.4.2.3).
///
/// For luma blocks of size >= 8, if the mode is "far enough" from pure
/// horizontal (10) or pure vertical (26), apply either:
///   - Strong intra smoothing: linear interpolation (32x32 blocks only)
///   - Standard smoothing: [1, 2, 1] / 4 low-pass filter
///
/// DC mode (1) and 4x4 blocks are never filtered.
fn filter_reference_samples(
    refs: &mut RefSamples,
    n_tb_s: u32,
    mode: u8,
    comp: Component,
    strong_intra_smoothing_enabled: bool,
    bit_depth: u8,
) {
    // Only luma for 4:2:0 (chroma_format_idc != 3)
    if comp != Component::Y {
        return;
    }
    // DC and 4x4 blocks are never filtered
    if mode == 1 || n_tb_s < 8 {
        return;
    }

    let log2n = log2_floor(n_tb_s);
    let thresh_idx = (log2n - 3) as usize;
    if thresh_idx >= INTRA_HOR_VER_DIST_THRESH.len() {
        return;
    }

    let dist_vert = (mode as i32 - 26).unsigned_abs();
    let dist_horiz = (mode as i32 - 10).unsigned_abs();
    let min_dist = dist_vert.min(dist_horiz);

    if min_dist <= INTRA_HOR_VER_DIST_THRESH[thresh_idx] {
        return;
    }

    let size2 = (2 * n_tb_s) as usize; // length of top[] and left[]
    let tl = refs.top_left as i32;

    // Try strong intra smoothing first (32x32 luma only)
    if strong_intra_smoothing_enabled && log2n == 5 {
        let threshold = 1i32 << (bit_depth.saturating_sub(5));
        let top_dev = (tl + refs.top[size2 - 1] as i32
            - 2 * refs.top[(n_tb_s - 1) as usize] as i32)
            .abs();
        let left_dev = (tl + refs.left[size2 - 1] as i32
            - 2 * refs.left[(n_tb_s - 1) as usize] as i32)
            .abs();

        if top_dev < threshold && left_dev < threshold {
            let p_top_end = refs.top[size2 - 1] as i32;
            let p_left_end = refs.left[size2 - 1] as i32;

            // Interpolate top[0..size2-2], preserve top[size2-1]
            for i in 0..size2 - 1 {
                let k = (i as i32) + 1;
                refs.top[i] = (((size2 as i32 - k) * tl
                    + k * p_top_end
                    + (size2 as i32 >> 1))
                    / size2 as i32) as i16;
            }
            // Interpolate left[0..size2-2], preserve left[size2-1]
            for j in 0..size2 - 1 {
                let k = (j as i32) + 1;
                refs.left[j] = (((size2 as i32 - k) * tl
                    + k * p_left_end
                    + (size2 as i32 >> 1))
                    / size2 as i32) as i16;
            }
            return;
        }
    }

    // Standard [1, 2, 1] / 4 low-pass filter
    let mut filtered_top = vec![0i16; size2];
    let mut filtered_left = vec![0i16; size2];

    // Last element is preserved
    filtered_left[size2 - 1] = refs.left[size2 - 1];
    filtered_top[size2 - 1] = refs.top[size2 - 1];

    // Filter left: left[i] = (left[i+1] + 2*left[i] + left[i-1] + 2) >> 2
    // where left[-1] = top_left
    for i in (0..size2 - 1).rev() {
        let above = if i == 0 {
            tl
        } else {
            refs.left[i - 1] as i32
        };
        filtered_left[i] =
            ((refs.left[i + 1] as i32 + 2 * refs.left[i] as i32 + above + 2) >> 2) as i16;
    }

    // Filtered top_left = (left[0] + 2*top_left + top[0] + 2) >> 2
    let filtered_tl =
        ((refs.left[0] as i32 + 2 * tl + refs.top[0] as i32 + 2) >> 2) as i16;

    // Filter top: top[i] = (top[i+1] + 2*top[i] + top[i-1] + 2) >> 2
    // where top[-1] = top_left
    for i in (0..size2 - 1).rev() {
        let left_of = if i == 0 {
            tl
        } else {
            refs.top[i - 1] as i32
        };
        filtered_top[i] =
            ((refs.top[i + 1] as i32 + 2 * refs.top[i] as i32 + left_of + 2) >> 2) as i16;
    }

    refs.top_left = filtered_tl;
    refs.top = filtered_top;
    refs.left = filtered_left;
}

// ---------------------------------------------------------------------------
// Mode 0 — Planar (H.265 8.4.4.2.4)
// ---------------------------------------------------------------------------

/// Planar intra prediction: bilinear interpolation from the four edge reference
/// samples.
fn predict_planar(refs: &RefSamples, n_tb_s: u32) -> Vec<i16> {
    let n = n_tb_s as i32;
    let log2n = log2_floor(n_tb_s);
    let shift = log2n + 1;
    let size = n_tb_s as usize;
    let mut pred = vec![0i16; size * size];

    let p_top_right = refs.top[n as usize] as i32;  // p[nTbS][-1]
    let p_bottom_left = refs.left[n as usize] as i32; // p[-1][nTbS]

    for y in 0..size {
        for x in 0..size {
            let xi = x as i32;
            let yi = y as i32;
            let val = (n - 1 - xi) * (refs.left[y] as i32)
                + (xi + 1) * p_top_right
                + (n - 1 - yi) * (refs.top[x] as i32)
                + (yi + 1) * p_bottom_left
                + n;
            pred[y * size + x] = (val >> shift) as i16;
        }
    }
    pred
}

// ---------------------------------------------------------------------------
// Mode 1 — DC (H.265 8.4.4.2.5)
// ---------------------------------------------------------------------------

/// DC intra prediction: constant block value from average of top and left
/// reference samples, with boundary filtering on the first row and column.
fn predict_dc(refs: &RefSamples, n_tb_s: u32, comp: Component) -> Vec<i16> {
    let n = n_tb_s as usize;
    let log2n = log2_floor(n_tb_s);

    // Compute DC value: average of top[0..n-1] and left[0..n-1]
    let mut sum: i32 = 0;
    for i in 0..n {
        sum += refs.top[i] as i32;
    }
    for j in 0..n {
        sum += refs.left[j] as i32;
    }
    let dc_val = ((sum + n as i32) >> (log2n + 1)) as i16;


    let mut pred = vec![dc_val; n * n];

    // DC boundary filtering applies to luma only (H.265 8.4.4.2.5 note)
    if comp == Component::Y && n_tb_s < 32 {
        // pred[0][0]
        pred[0] = ((refs.left[0] as i32 + 2 * dc_val as i32 + refs.top[0] as i32 + 2) >> 2)
            as i16;

        // First row: pred[x][0] for x > 0
        for x in 1..n {
            pred[x] = ((refs.top[x] as i32 + 3 * dc_val as i32 + 2) >> 2) as i16;
        }

        // First column: pred[0][y] for y > 0
        for y in 1..n {
            pred[y * n] = ((refs.left[y] as i32 + 3 * dc_val as i32 + 2) >> 2) as i16;
        }
    }

    pred
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Predict a block using HEVC intra prediction.
///
/// # Parameters
/// - `pic`:   Reconstructed picture containing already-decoded neighbor samples.
/// - `x`, `y`: Block position in **luma** sample coordinates.
/// - `size`:  Block side length in samples (4, 8, 16, or 32).
/// - `mode`:  Intra prediction mode (0..=34).
/// - `component`: Color component being predicted.
/// - `strong_intra_smoothing`: SPS flag enabling reference sample smoothing.
///
/// # Returns
/// `size * size` predicted samples in row-major order.
///
/// # Panics
/// Panics if `mode > 34` or `size` is not a power of two in {4, 8, 16, 32}.
pub fn predict_intra(
    pic: &Picture,
    x: u32,
    y: u32,
    size: u32,
    mode: u8,
    component: Component,
    strong_intra_smoothing: bool,
) -> Vec<i16> {
    assert!(mode <= 34, "Invalid intra mode {mode}; must be 0..=34");
    assert!(
        matches!(size, 4 | 8 | 16 | 32),
        "Invalid block size {size}; must be 4, 8, 16, or 32"
    );

    // For chroma components in 4:2:0, convert luma coordinates to chroma
    let (bx, by, bs) = match component {
        Component::Y => (x, y, size),
        Component::Cb | Component::Cr => (x / 2, y / 2, size / 2.max(1)),
    };
    // Ensure chroma block size is at least 2 (minimum meaningful size)
    let bs = bs.max(2);

    let mut refs = build_reference_samples(pic, bx, by, bs, component);

    // Reference sample filtering (H.265 8.4.4.2.3)
    filter_reference_samples(
        &mut refs,
        bs,
        mode,
        component,
        strong_intra_smoothing,
        pic.bit_depth,
    );

    // Edge filter for modes 10/26: luma only, block size < 32 (H.265 8.4.4.2.7)
    let edge_filter = component == Component::Y && bs < 32;

    match mode {
        0 => predict_planar(&refs, bs),
        1 => predict_dc(&refs, bs, component),
        2..=34 => {
            predict_angular_with_offset(&refs, bs, mode, edge_filter, pic.bit_depth)
        }
        _ => unreachable!(),
    }
}

/// Angular prediction that correctly handles the offset for negative-angle modes.
/// `apply_edge_filter` should be true only for luma blocks smaller than 32.
fn predict_angular_with_offset(refs: &RefSamples, n_tb_s: u32, mode: u8, apply_edge_filter: bool, bit_depth: u8) -> Vec<i16> {
    let n = n_tb_s as usize;
    let angle_idx = (mode - 2) as usize;
    let angle = INTRA_PRED_ANGLE[angle_idx];

    let mut pred = vec![0i16; n * n];

    if mode >= 18 {
        // Vertical-ish modes
        if angle >= 0 {
            let mut ref_main = Vec::with_capacity(2 * n + 1);
            ref_main.push(refs.top_left);
            ref_main.extend_from_slice(&refs.top[..2 * n]);

            for y in 0..n {
                let delta_pos = ((y as i32) + 1) * angle;
                let i_idx = delta_pos >> 5;
                let i_fact = delta_pos & 31;

                for x in 0..n {
                    let ri = (x as i32) + i_idx + 1;
                    if i_fact == 0 {
                        pred[y * n + x] = ref_main[ri as usize];
                    } else {
                        let a = ref_main[ri as usize] as i32;
                        let b = ref_main[(ri + 1) as usize] as i32;
                        pred[y * n + x] = (((32 - i_fact) * a + i_fact * b + 16) >> 5) as i16;
                    }
                }
            }
        } else {
            // Negative angle: build extended ref with left-projected samples
            let inv_a = inv_angle_for(angle);
            let extra = (((n as i32) * angle.unsigned_abs() as i32 + 31) >> 5)
                .min(n as i32) as usize;

            let mut ref_main = vec![0i16; extra + 1 + 2 * n];
            ref_main[extra] = refs.top_left;
            for i in 0..2 * n {
                ref_main[extra + 1 + i] = refs.top[i];
            }
            for k in 1..=extra {
                let left_idx = ((k as i32 * inv_a + 128) >> 8) - 1;
                let clamped = left_idx.clamp(0, (2 * n as i32) - 1) as usize;
                ref_main[extra - k] = refs.left[clamped];
            }

            let off = extra as i32;
            for y in 0..n {
                let delta_pos = ((y as i32) + 1) * angle;
                let i_idx = delta_pos >> 5;
                let i_fact = delta_pos & 31;

                for x in 0..n {
                    let ri = (x as i32) + i_idx + 1 + off;
                    if i_fact == 0 {
                        pred[y * n + x] = ref_main[ri as usize];
                    } else {
                        let a = ref_main[ri as usize] as i32;
                        let b = ref_main[(ri + 1) as usize] as i32;
                        pred[y * n + x] = (((32 - i_fact) * a + i_fact * b + 16) >> 5) as i16;
                    }
                }
            }
        }

        // Post-filter for pure vertical (mode 26) — luma only, size < 32
        if mode == 26 && apply_edge_filter {
            for y in 0..n {
                pred[y * n] = clip_to_bit_depth(
                    refs.top[0] as i32 + ((refs.left[y] as i32 - refs.top_left as i32) >> 1),
                    bit_depth as u32,
                );
            }
        }
    } else {
        // Horizontal-ish modes (2..=17)
        if angle >= 0 {
            let mut ref_main = Vec::with_capacity(2 * n + 1);
            ref_main.push(refs.top_left);
            ref_main.extend_from_slice(&refs.left[..2 * n]);

            for y in 0..n {
                for x in 0..n {
                    let delta_pos = ((x as i32) + 1) * angle;
                    let i_idx = delta_pos >> 5;
                    let i_fact = delta_pos & 31;

                    let ri = (y as i32) + i_idx + 1;
                    if i_fact == 0 {
                        pred[y * n + x] = ref_main[ri as usize];
                    } else {
                        let a = ref_main[ri as usize] as i32;
                        let b = ref_main[(ri + 1) as usize] as i32;
                        pred[y * n + x] = (((32 - i_fact) * a + i_fact * b + 16) >> 5) as i16;
                    }
                }
            }
        } else {
            // Negative angle: build extended ref with top-projected samples
            let inv_a = inv_angle_for(angle);
            let extra = (((n as i32) * angle.unsigned_abs() as i32 + 31) >> 5)
                .min(n as i32) as usize;

            let mut ref_main = vec![0i16; extra + 1 + 2 * n];
            ref_main[extra] = refs.top_left;
            for j in 0..2 * n {
                ref_main[extra + 1 + j] = refs.left[j];
            }
            for k in 1..=extra {
                let top_idx = ((k as i32 * inv_a + 128) >> 8) - 1;
                let clamped = top_idx.clamp(0, (2 * n as i32) - 1) as usize;
                ref_main[extra - k] = refs.top[clamped];
            }

            let off = extra as i32;
            for y in 0..n {
                for x in 0..n {
                    let delta_pos = ((x as i32) + 1) * angle;
                    let i_idx = delta_pos >> 5;
                    let i_fact = delta_pos & 31;

                    let ri = (y as i32) + i_idx + 1 + off;
                    if i_fact == 0 {
                        pred[y * n + x] = ref_main[ri as usize];
                    } else {
                        let a = ref_main[ri as usize] as i32;
                        let b = ref_main[(ri + 1) as usize] as i32;
                        pred[y * n + x] = (((32 - i_fact) * a + i_fact * b + 16) >> 5) as i16;
                    }
                }
            }
        }

        // Post-filter for pure horizontal (mode 10) — luma only, size < 32
        if mode == 10 && apply_edge_filter {
            for x in 0..n {
                pred[x] = clip_to_bit_depth(
                    refs.left[0] as i32 + ((refs.top[x] as i32 - refs.top_left as i32) >> 1),
                    bit_depth as u32,
                );
            }
        }
    }

    pred
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

/// Floor of log2 for power-of-two values (4 → 2, 8 → 3, 16 → 4, 32 → 5).
fn log2_floor(n: u32) -> u32 {
    31 - n.leading_zeros()
}

/// Clip a value to the range representable by `bit_depth` bits (signed i16).
fn clip_to_bit_depth(val: i32, bit_depth: u32) -> i16 {
    let max = (1i32 << bit_depth) - 1;
    val.clamp(0, max) as i16
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a picture and fill a region of the luma plane so that
    /// reference samples around a block at `(bx, by)` of size `n` are available.
    fn make_test_pic(width: u32, height: u32) -> Picture {
        Picture::new(width, height, 8)
    }

    fn fill_luma_constant(pic: &mut Picture, val: i16) {
        for s in pic.y_mut().iter_mut() {
            *s = val;
        }
    }

    fn fill_cb_constant(pic: &mut Picture, val: i16) {
        for s in pic.cb_mut().iter_mut() {
            *s = val;
        }
    }

    fn fill_cr_constant(pic: &mut Picture, val: i16) {
        for s in pic.cr_mut().iter_mut() {
            *s = val;
        }
    }

    fn fill_luma_ramp(pic: &mut Picture) {
        let w = pic.width;
        let h = pic.height;
        for y in 0..h {
            for x in 0..w {
                pic.set_y(x, y, (x + y * w) as i16);
            }
        }
    }

    // ------- log2_floor -------

    #[test]
    fn log2_floor_values() {
        assert_eq!(log2_floor(4), 2);
        assert_eq!(log2_floor(8), 3);
        assert_eq!(log2_floor(16), 4);
        assert_eq!(log2_floor(32), 5);
    }

    // ------- clip_to_bit_depth -------

    #[test]
    fn clip_to_bit_depth_8() {
        assert_eq!(clip_to_bit_depth(-5, 8), 0);
        assert_eq!(clip_to_bit_depth(128, 8), 128);
        assert_eq!(clip_to_bit_depth(300, 8), 255);
    }

    #[test]
    fn clip_to_bit_depth_10() {
        assert_eq!(clip_to_bit_depth(1023, 10), 1023);
        assert_eq!(clip_to_bit_depth(1024, 10), 1023);
    }

    // ------- Reference sample construction -------

    #[test]
    fn ref_samples_all_available() {
        let mut pic = make_test_pic(16, 16);
        fill_luma_constant(&mut pic, 100);
        pic.set_y(3, 3, 42); // block at (4,4) has this as top-left neighbor

        let refs = build_reference_samples(&pic, 4, 4, 4, Component::Y);
        // top_left = p[-1][-1] = p[3][3] = 42
        assert_eq!(refs.top_left, 42);
        // top[0] = p[4][-1] = p[4][3] = 100
        assert_eq!(refs.top[0], 100);
        // left[0] = p[-1][4] = p[3][4] = 100
        assert_eq!(refs.left[0], 100);
        assert_eq!(refs.top.len(), 8); // 2 * 4
        assert_eq!(refs.left.len(), 8);
    }

    #[test]
    fn ref_samples_top_left_block_substitutes() {
        // Block at (0,0): all neighbors are out of bounds
        let mut pic = make_test_pic(8, 8);
        fill_luma_constant(&mut pic, 55);

        let refs = build_reference_samples(&pic, 0, 0, 4, Component::Y);
        // No neighbors available → substitution with mid-range (1 << 3 = 128 for 8-bit)
        // Actually, the fallback is 1 << (bit_depth - 1) = 128
        assert_eq!(refs.top_left, 128);
        for v in &refs.top {
            assert_eq!(*v, 128);
        }
        for v in &refs.left {
            assert_eq!(*v, 128);
        }
    }

    #[test]
    fn ref_samples_left_edge_partial() {
        // Block at (0, 4): left neighbors unavailable, top available
        let mut pic = make_test_pic(16, 16);
        fill_luma_constant(&mut pic, 80);

        let refs = build_reference_samples(&pic, 0, 4, 4, Component::Y);
        // top_left = p[-1][3] → x=-1, out of bounds → substituted
        // top[i] = p[i][3] → in bounds, = 80
        // left[j] = p[-1][4+j] → x=-1, out of bounds → substituted
        // Substitution fills from the first available sample in scan order.
        // Scan: left (bottom to top) all None, top_left None, top[0..7] all Some(80).
        // So fallback chain: first available = top[0] = 80.
        assert_eq!(refs.top[0], 80);
        assert_eq!(refs.top_left, 80);
        for v in &refs.left {
            assert_eq!(*v, 80);
        }
    }

    // ------- Mode 0 — Planar -------

    #[test]
    fn planar_constant_block() {
        // If all reference samples are the same value, planar produces that value.
        let mut pic = make_test_pic(16, 16);
        fill_luma_constant(&mut pic, 100);

        let pred = predict_intra(&pic, 4, 4, 4, 0, Component::Y, false);
        assert_eq!(pred.len(), 16);
        for v in &pred {
            assert_eq!(*v, 100);
        }
    }

    #[test]
    fn planar_output_length() {
        let mut pic = make_test_pic(32, 32);
        fill_luma_constant(&mut pic, 50);
        let pred = predict_intra(&pic, 8, 8, 8, 0, Component::Y, false);
        assert_eq!(pred.len(), 64);
    }

    // ------- Mode 1 — DC -------

    #[test]
    fn dc_constant_block() {
        let mut pic = make_test_pic(16, 16);
        fill_luma_constant(&mut pic, 80);

        let pred = predict_intra(&pic, 4, 4, 4, 1, Component::Y, false);
        assert_eq!(pred.len(), 16);
        // DC of all-80 refs = 80. Boundary filtering on luma:
        // pred[0][0] = (80 + 2*80 + 80 + 2) >> 2 = 322 >> 2 = 80
        // So all samples should be 80.
        for v in &pred {
            assert_eq!(*v, 80);
        }
    }

    #[test]
    fn dc_chroma_no_filter() {
        // DC on chroma should NOT apply boundary filtering
        let mut pic = make_test_pic(16, 16);
        fill_luma_constant(&mut pic, 50);
        fill_cb_constant(&mut pic, 60);

        let pred = predict_intra(&pic, 8, 8, 4, 1, Component::Cb, false);
        // Chroma block at (4,4) size 2 (from 4/2), but our min is 2
        // All chroma refs are 60, DC = 60, no filtering
        for v in &pred {
            assert_eq!(*v, 60);
        }
    }

    // ------- Angular modes -------

    #[test]
    fn mode26_pure_vertical_constant() {
        let mut pic = make_test_pic(16, 16);
        fill_luma_constant(&mut pic, 70);

        let pred = predict_intra(&pic, 4, 4, 4, 26, Component::Y, false);
        assert_eq!(pred.len(), 16);
        for v in &pred {
            assert_eq!(*v, 70);
        }
    }

    #[test]
    fn mode10_pure_horizontal_constant() {
        let mut pic = make_test_pic(16, 16);
        fill_luma_constant(&mut pic, 90);

        let pred = predict_intra(&pic, 4, 4, 4, 10, Component::Y, false);
        assert_eq!(pred.len(), 16);
        for v in &pred {
            assert_eq!(*v, 90);
        }
    }

    #[test]
    fn mode26_pure_vertical_copies_top_row() {
        // Pure vertical: each row should be a copy of the top reference row.
        let mut pic = make_test_pic(16, 16);
        fill_luma_ramp(&mut pic);

        let pred = predict_intra(&pic, 4, 4, 4, 26, Component::Y, false);
        // Top reference row: p[4..7][3] (row 3). Since fill_luma_ramp sets
        // pixel (x,y) = x + y*16, row 3 = [48..63], so p[4][3]=52, p[5]=53, etc.
        // Mode 26 post-filter modifies column 0, but for constant-neighbor-
        // difference situations, let's just check that rows 1-3 columns 1-3
        // replicate the top reference.
        let top_ref = [
            pic.y_at(4, 3),
            pic.y_at(5, 3),
            pic.y_at(6, 3),
            pic.y_at(7, 3),
        ];
        // Row 1, columns 1..3 (unfiltered)
        assert_eq!(pred[1 * 4 + 1], top_ref[1]);
        assert_eq!(pred[1 * 4 + 2], top_ref[2]);
        assert_eq!(pred[1 * 4 + 3], top_ref[3]);
    }

    #[test]
    fn mode10_pure_horizontal_copies_left_column() {
        let mut pic = make_test_pic(16, 16);
        fill_luma_ramp(&mut pic);

        let pred = predict_intra(&pic, 4, 4, 4, 10, Component::Y, false);
        let left_ref = [
            pic.y_at(3, 4),
            pic.y_at(3, 5),
            pic.y_at(3, 6),
            pic.y_at(3, 7),
        ];
        // Row 1-3, columns 1-3 (unfiltered)
        assert_eq!(pred[1 * 4 + 1], left_ref[1]);
        assert_eq!(pred[2 * 4 + 2], left_ref[2]);
        assert_eq!(pred[3 * 4 + 3], left_ref[3]);
    }

    // ------- Angle table sanity -------

    #[test]
    fn angle_table_length() {
        assert_eq!(INTRA_PRED_ANGLE.len(), 33);
    }

    #[test]
    fn angle_table_symmetry() {
        // Modes 2 and 34 both have angle magnitude 32
        assert_eq!(INTRA_PRED_ANGLE[0], 32);
        assert_eq!(INTRA_PRED_ANGLE[32], 32);
        // Mode 10 (index 8) and mode 26 (index 24) have angle 0
        assert_eq!(INTRA_PRED_ANGLE[8], 0);
        assert_eq!(INTRA_PRED_ANGLE[24], 0);
    }

    #[test]
    fn inv_angle_lookup() {
        assert_eq!(inv_angle_for(-2), 256);
        assert_eq!(inv_angle_for(-5), 315);
        assert_eq!(inv_angle_for(-9), 390);
        assert_eq!(inv_angle_for(-13), 512);
        assert_eq!(inv_angle_for(-17), 630);
        assert_eq!(inv_angle_for(-21), 819);
        assert_eq!(inv_angle_for(-26), 1024);
    }

    // ------- Negative-angle modes -------

    #[test]
    fn mode11_negative_angle_vertical() {
        // Mode 11 has angle index 9, angle = -2 (a slightly off-vertical mode
        // that still leans horizontal — wait, mode 11 < 18 so it's horizontal-ish)
        // Let's test mode 19 which is vertical-ish with angle index 17 = -26
        let mut pic = make_test_pic(32, 32);
        fill_luma_constant(&mut pic, 120);

        let pred = predict_intra(&pic, 8, 8, 4, 19, Component::Y, false);
        assert_eq!(pred.len(), 16);
        // Constant reference → constant prediction
        for v in &pred {
            assert_eq!(*v, 120);
        }
    }

    #[test]
    fn mode11_negative_angle_horizontal() {
        let mut pic = make_test_pic(32, 32);
        fill_luma_constant(&mut pic, 110);

        let pred = predict_intra(&pic, 8, 8, 4, 11, Component::Y, false);
        assert_eq!(pred.len(), 16);
        for v in &pred {
            assert_eq!(*v, 110);
        }
    }

    // ------- All modes produce correct output length -------

    #[test]
    fn all_modes_correct_length_4x4() {
        let mut pic = make_test_pic(32, 32);
        fill_luma_constant(&mut pic, 100);

        for mode in 0..=34u8 {
            let pred = predict_intra(&pic, 8, 8, 4, mode, Component::Y, false);
            assert_eq!(pred.len(), 16, "mode {mode} produced wrong length");
        }
    }

    #[test]
    fn all_modes_correct_length_8x8() {
        let mut pic = make_test_pic(32, 32);
        fill_luma_constant(&mut pic, 100);

        for mode in 0..=34u8 {
            let pred = predict_intra(&pic, 8, 8, 8, mode, Component::Y, false);
            assert_eq!(pred.len(), 64, "mode {mode} produced wrong length");
        }
    }

    #[test]
    fn all_modes_correct_length_16x16() {
        let mut pic = make_test_pic(64, 64);
        fill_luma_constant(&mut pic, 100);

        for mode in 0..=34u8 {
            let pred = predict_intra(&pic, 16, 16, 16, mode, Component::Y, false);
            assert_eq!(pred.len(), 256, "mode {mode} produced wrong length");
        }
    }

    #[test]
    fn all_modes_correct_length_32x32() {
        let mut pic = make_test_pic(128, 128);
        fill_luma_constant(&mut pic, 100);

        for mode in 0..=34u8 {
            let pred = predict_intra(&pic, 32, 32, 32, mode, Component::Y, false);
            assert_eq!(pred.len(), 1024, "mode {mode} produced wrong length");
        }
    }

    // ------- Strong intra smoothing -------

    #[test]
    fn strong_smoothing_constant_refs_unchanged() {
        // When all refs are the same value, smoothing should not change anything
        let mut pic = make_test_pic(128, 128);
        fill_luma_constant(&mut pic, 100);

        let pred_no = predict_intra(&pic, 32, 32, 32, 0, Component::Y, false);
        let pred_yes = predict_intra(&pic, 32, 32, 32, 0, Component::Y, true);
        assert_eq!(pred_no, pred_yes);
    }

    // ------- Chroma prediction -------

    #[test]
    fn chroma_block_size_halved() {
        let mut pic = make_test_pic(32, 32);
        fill_luma_constant(&mut pic, 100);
        fill_cb_constant(&mut pic, 80);
        fill_cr_constant(&mut pic, 90);

        // Requesting 4x4 on Cb → chroma block is 2x2 (minimum 2)
        let pred = predict_intra(&pic, 8, 8, 4, 1, Component::Cb, false);
        assert_eq!(pred.len(), 4); // 2*2
    }

    #[test]
    fn chroma_8x8_becomes_4x4() {
        let mut pic = make_test_pic(32, 32);
        fill_cb_constant(&mut pic, 60);

        let pred = predict_intra(&pic, 8, 8, 8, 0, Component::Cb, false);
        assert_eq!(pred.len(), 16); // 4*4
    }

    // ------- Edge: block at picture boundary -------

    #[test]
    fn block_at_top_edge() {
        let mut pic = make_test_pic(16, 16);
        fill_luma_constant(&mut pic, 50);

        // Block at (4, 0): top neighbors are out of bounds
        let pred = predict_intra(&pic, 4, 0, 4, 1, Component::Y, false);
        assert_eq!(pred.len(), 16);
        // Should not panic; substitution fills unavailable refs
    }

    #[test]
    fn block_at_bottom_right_corner() {
        let mut pic = make_test_pic(16, 16);
        fill_luma_constant(&mut pic, 75);

        // Block at (12, 12): bottom-left and top-right extensions partially out of bounds
        let pred = predict_intra(&pic, 12, 12, 4, 0, Component::Y, false);
        assert_eq!(pred.len(), 16);
    }

    // ------- Mode boundary: mode 18 (diagonal) -------

    #[test]
    fn mode18_diagonal_constant() {
        let mut pic = make_test_pic(32, 32);
        fill_luma_constant(&mut pic, 60);

        let pred = predict_intra(&pic, 8, 8, 4, 18, Component::Y, false);
        assert_eq!(pred.len(), 16);
        for v in &pred {
            assert_eq!(*v, 60);
        }
    }

    // ------- Mode 2 and mode 34 (extreme diagonals) -------

    #[test]
    fn mode2_extreme_horizontal_constant() {
        let mut pic = make_test_pic(32, 32);
        fill_luma_constant(&mut pic, 45);

        let pred = predict_intra(&pic, 8, 8, 4, 2, Component::Y, false);
        assert_eq!(pred.len(), 16);
        for v in &pred {
            assert_eq!(*v, 45);
        }
    }

    #[test]
    fn mode34_extreme_vertical_constant() {
        let mut pic = make_test_pic(32, 32);
        fill_luma_constant(&mut pic, 200);

        let pred = predict_intra(&pic, 8, 8, 4, 34, Component::Y, false);
        assert_eq!(pred.len(), 16);
        for v in &pred {
            assert_eq!(*v, 200);
        }
    }

    // ------- Panic on invalid inputs -------

    #[test]
    #[should_panic(expected = "Invalid intra mode")]
    fn invalid_mode_panics() {
        let pic = make_test_pic(16, 16);
        predict_intra(&pic, 4, 4, 4, 35, Component::Y, false);
    }

    #[test]
    #[should_panic(expected = "Invalid block size")]
    fn invalid_size_panics() {
        let pic = make_test_pic(16, 16);
        predict_intra(&pic, 4, 4, 6, 0, Component::Y, false);
    }
}
