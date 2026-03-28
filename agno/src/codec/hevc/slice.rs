use anyhow::Result;

use super::bitstream::BitReader;
use super::intra::{Component, predict_intra};
use super::params::{Pps, Sps};
use super::picture::{CtuSaoParams, Picture, SaoParams};
use super::transform;

#[cfg(feature = "cabac-trace")]
macro_rules! cabac_trace {
    ($($arg:tt)*) => { eprintln!($($arg)*); }
}
#[cfg(not(feature = "cabac-trace"))]
macro_rules! cabac_trace {
    ($($arg:tt)*) => {};
}

// ---------------------------------------------------------------------------
// CABAC engine (ITU-T H.265 section 9.3)
// ---------------------------------------------------------------------------

const TRANS_IDX_LPS: [u8; 64] = [
    0, 0, 1, 2, 2, 4, 4, 5, 6, 7, 8, 9, 9, 11, 11, 12, 13, 13, 15, 15, 16, 16, 18, 18, 19, 19, 21,
    21, 22, 22, 23, 24, 24, 25, 26, 26, 27, 27, 28, 29, 29, 30, 30, 30, 31, 32, 32, 33, 33, 33, 34,
    34, 35, 35, 35, 36, 36, 36, 37, 37, 37, 38, 38, 63,
];

const TRANS_IDX_MPS: [u8; 64] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50,
    51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 62, 63,
];

const RANGE_TAB_LPS: [[u16; 4]; 64] = [
    [128, 176, 208, 240],
    [128, 167, 197, 227],
    [128, 158, 187, 216],
    [123, 150, 178, 205],
    [116, 142, 169, 195],
    [111, 135, 160, 185],
    [105, 128, 152, 175],
    [100, 122, 144, 166],
    [95, 116, 137, 158],
    [90, 110, 130, 150],
    [85, 104, 123, 142],
    [81, 99, 117, 135],
    [77, 94, 111, 128],
    [73, 89, 105, 122],
    [69, 85, 100, 116],
    [66, 80, 95, 110],
    [62, 76, 90, 104],
    [59, 72, 86, 99],
    [56, 69, 81, 94],
    [53, 65, 77, 89],
    [51, 62, 73, 85],
    [48, 59, 69, 80],
    [46, 56, 66, 76],
    [43, 53, 63, 72],
    [41, 50, 59, 69],
    [39, 48, 56, 65],
    [37, 45, 54, 62],
    [35, 43, 51, 59],
    [33, 41, 48, 56],
    [32, 39, 46, 53],
    [30, 37, 43, 50],
    [29, 35, 41, 48],
    [27, 33, 39, 45],
    [26, 31, 37, 43],
    [24, 30, 35, 41],
    [23, 28, 33, 39],
    [22, 27, 32, 37],
    [21, 26, 30, 35],
    [20, 24, 29, 33],
    [19, 23, 27, 31],
    [18, 22, 26, 30],
    [17, 21, 25, 28],
    [16, 20, 23, 27],
    [15, 19, 22, 25],
    [14, 18, 21, 24],
    [14, 17, 20, 23],
    [13, 16, 19, 22],
    [12, 15, 18, 21],
    [12, 14, 17, 20],
    [11, 14, 16, 19],
    [11, 13, 15, 18],
    [10, 12, 15, 17],
    [10, 12, 14, 16],
    [9, 11, 13, 15],
    [9, 11, 12, 14],
    [8, 10, 12, 14],
    [8, 9, 11, 13],
    [7, 9, 11, 12],
    [7, 9, 10, 12],
    [7, 8, 10, 11],
    [6, 8, 9, 11],
    [6, 7, 9, 10],
    [6, 7, 8, 9],
    [2, 2, 2, 2],
];

struct CabacCtx {
    state: u8,
    mps: u8,
}

struct CabacReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u8,
    range: u32,
    offset: u32,
    ctxs: Vec<CabacCtx>,
    dec_count: u32,
    byp_count: u32,
    trm_count: u32,
}

impl<'a> CabacReader<'a> {
    fn new(data: &'a [u8], byte_offset: usize) -> Self {
        let mut r = Self {
            data,
            pos: byte_offset,
            bit: 0,
            range: 510,
            offset: 0,
            ctxs: Vec::new(),
            dec_count: 0,
            byp_count: 0,
            trm_count: 0,
        };
        // Read initial 9 bits for ivlOffset
        r.offset = r.read_raw(9);
        r
    }

    fn read_raw(&mut self, n: u32) -> u32 {
        let mut val = 0u32;
        for _ in 0..n {
            if self.pos < self.data.len() {
                val = (val << 1) | ((self.data[self.pos] >> (7 - self.bit)) & 1) as u32;
            } else {
                val <<= 1;
            }
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        val
    }

    fn init_contexts(&mut self, slice_qp: i32) {
        // H.265 Section 9.3.2.2 — per-context initialization.
        // Values from FFmpeg libavcodec/hevc/cabac.c init_values[0] (I-slice),
        // mapped to this decoder's CTX_* index layout.
        //
        // Each value encodes (slope, offset):
        //   m = (initValue >> 4) * 5 - 45
        //   n = ((initValue & 15) << 3) - 16
        //   preCtxState = Clip3(1, 126, ((m * Clip3(0,51,SliceQpY)) >> 4) + n)
        #[rustfmt::skip]
        const INIT_VALUES: [u8; 256] = [
            // CTX 0..2: split_cu_flag (Table 9-5)
            139, 141, 157,
            // CTX 3..6: padding (code uses CTX_PART_MODE=7, these are unused)
            154, 154, 154, 154,
            // CTX 7..10: part_mode (Table 9-6)
            184, 154, 154, 154,
            // CTX 11: prev_intra_luma_pred_flag (Table 9-7)
            184,
            // CTX 12: intra_chroma_pred_mode (Table 9-8)
            63,
            // CTX 13..15: split_transform_flag (Table 9-9)
            153, 138, 138,
            // CTX 16..17: cbf_luma (Table 9-10)
            111, 141,
            // CTX 18..21: cbf_cb/cr (Table 9-11)
            94, 138, 182, 154,
            // CTX 22..39: last_sig_coeff_x_prefix (Table 9-12, 18 values)
            110, 110, 124, 125, 140, 153, 125, 127,
            140, 109, 111, 143, 127, 111,  79, 108,
            123,  63,
            // CTX 40..57: last_sig_coeff_y_prefix (Table 9-13, 18 values)
            110, 110, 124, 125, 140, 153, 125, 127,
            140, 109, 111, 143, 127, 111,  79, 108,
            123,  63,
            // CTX 58..61: coded_sub_block_flag (Table 9-14)
            91, 171, 134, 141,
            // CTX 62..105: sig_coeff_flag (Table 9-15, 44 values)
            111, 111, 125, 110, 110,  94, 124, 108,
            124, 107, 125, 141, 179, 153, 125, 107,
            125, 141, 179, 153, 125, 107, 125, 141,
            179, 153, 125, 140, 139, 182, 182, 152,
            136, 152, 136, 153, 136, 139, 111, 136,
            139, 111, 141, 111,
            // CTX 106..129: coeff_abs_level_greater1_flag (Table 9-16, 24 values)
            140,  92, 137, 138, 140, 152, 138, 139,
            153,  74, 149,  92, 139, 107, 122, 152,
            140, 179, 166, 182, 140, 227, 122, 197,
            // CTX 130..135: coeff_abs_level_greater2_flag (Table 9-17, 6 values)
            138, 153, 136, 167, 152, 152,
            // CTX 136: sao_merge_flag (Table 9-18)
            153,
            // CTX 137: sao_type_idx (Table 9-19)
            200,
            // CTX 138..139: cu_qp_delta_abs (init 154)
            // CTX 140..141: transform_skip_flag (init 139)
            154, 154, 139, 139, 154, 154, 154, 154, 154, 154,
            154, 154, 154, 154, 154, 154, 154, 154, 154, 154,
            154, 154, 154, 154, 154, 154, 154, 154, 154, 154,
            154, 154, 154, 154, 154, 154, 154, 154, 154, 154,
            154, 154, 154, 154, 154, 154, 154, 154, 154, 154,
            154, 154, 154, 154, 154, 154, 154, 154, 154, 154,
            154, 154, 154, 154, 154, 154, 154, 154, 154, 154,
            154, 154, 154, 154, 154, 154, 154, 154, 154, 154,
            154, 154, 154, 154, 154, 154, 154, 154, 154, 154,
            154, 154, 154, 154, 154, 154, 154, 154, 154, 154,
            154, 154, 154, 154, 154, 154, 154, 154, 154, 154,
            154, 154, 154, 154, 154, 154, 154, 154,
        ];

        self.ctxs.clear();
        for &iv in &INIT_VALUES {
            let iv = iv as i32;
            let slope = (iv >> 4) * 5 - 45;
            let off = ((iv & 15) << 3) - 16;
            let pre = ((slope * slice_qp.clamp(0, 51)) >> 4) + off;
            let pre = pre.clamp(1, 126);
            let (st, mp) = if pre <= 63 {
                ((63 - pre) as u8, 0u8)
            } else {
                ((pre - 64) as u8, 1u8)
            };
            self.ctxs.push(CabacCtx { state: st, mps: mp });
        }
    }

    fn renorm(&mut self) {
        while self.range < 256 {
            self.range <<= 1;
            self.offset = (self.offset << 1) | self.read_raw(1);
        }
    }

    fn reset_counters(&mut self) {
        self.dec_count = 0;
        self.byp_count = 0;
        self.trm_count = 0;
    }

    fn decode_decision(&mut self, ctx_idx: usize) -> u8 {
        self.dec_count += 1;
        let q = ((self.range >> 6) & 3) as usize;
        let lps_range = RANGE_TAB_LPS[self.ctxs[ctx_idx].state as usize][q] as u32;
        self.range -= lps_range;

        let st = self.ctxs[ctx_idx].state;
        let mp = self.ctxs[ctx_idx].mps;

        let decoded;
        if self.offset >= self.range {
            self.offset -= self.range;
            self.range = lps_range;
            decoded = 1 - mp;
            if st == 0 {
                self.ctxs[ctx_idx].mps = 1 - mp;
            }
            self.ctxs[ctx_idx].state = TRANS_IDX_LPS[st as usize];
            self.renorm();
        } else {
            self.ctxs[ctx_idx].state = TRANS_IDX_MPS[st as usize];
            self.renorm();
            decoded = mp;
        }
        cabac_trace!(
            "DEC ctx={} bit={} range={} offset={}",
            ctx_idx,
            decoded,
            self.range,
            self.offset
        );
        decoded
    }

    #[allow(clippy::let_and_return)]
    fn decode_bypass(&mut self) -> u8 {
        self.byp_count += 1;
        self.offset = (self.offset << 1) | self.read_raw(1);
        let bit = if self.offset >= self.range {
            self.offset -= self.range;
            1
        } else {
            0
        };
        cabac_trace!(
            "BYP bit={} range={} offset={}",
            bit,
            self.range,
            self.offset
        );
        bit
    }

    fn decode_bypass_bits(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.decode_bypass() as u32;
        }
        v
    }

    #[allow(clippy::let_and_return)]
    fn decode_terminate(&mut self) -> bool {
        self.trm_count += 1;
        self.range -= 2;
        let bit = if self.offset >= self.range {
            true
        } else {
            self.renorm();
            false
        };
        cabac_trace!(
            "TRM bit={} range={} offset={}",
            bit as u8,
            self.range,
            self.offset
        );
        bit
    }

    /// Save CABAC context state for WPP (H.265 Section 9.3.2.3).
    fn save_contexts(&self) -> Vec<(u8, u8)> {
        self.ctxs.iter().map(|c| (c.state, c.mps)).collect()
    }

    /// Restore CABAC context state from a saved snapshot (WPP row start).
    fn restore_contexts(&mut self, saved: &[(u8, u8)]) {
        for (c, &(st, mp)) in self.ctxs.iter_mut().zip(saved.iter()) {
            c.state = st;
            c.mps = mp;
        }
    }
}

// CABAC context indices
const CTX_SPLIT_CU: usize = 0;
const CTX_PART_MODE: usize = 7;
const CTX_PREV_INTRA_PRED: usize = 11;
const CTX_CHROMA_PRED: usize = 12;
const CTX_SPLIT_TF: usize = 13;
const CTX_CBF_LUMA: usize = 16;
const CTX_CBF_CHROMA: usize = 18;
const CTX_LAST_X: usize = 22;
const CTX_LAST_Y: usize = 40;
const CTX_CODED_SUB: usize = 58;
const CTX_SIG_COEFF: usize = 62;
const CTX_GT1: usize = 106;
const CTX_GT2: usize = 130;
const CTX_SAO_MERGE: usize = 136;
const CTX_SAO_TYPE: usize = 137;
const CTX_CU_QP_DELTA: usize = 138; // 2 contexts (138, 139), init value 154
const CTX_TRANSFORM_SKIP: usize = 140; // 2 contexts (140=luma, 141=chroma), init value 139

/// Mutable state for per-quantization-group QP delta tracking.
/// H.265 Section 7.4.9.10: cu_qp_delta_abs is coded once per QG.
struct QpState {
    /// Whether cu_qp_delta has been decoded in this quantization group.
    is_cu_qp_delta_coded: bool,
    /// Current QP (the QPY of the most recently decoded QG).
    current_qp: i32,
    /// QPY from the previous QG (H.265 8.6.1: lastQPYinPreviousQG).
    last_qp_in_previous_qg: i32,
    /// Top-left position of the current quantization group.
    current_qg_x: i32,
    current_qg_y: i32,
    /// Log2 of the minimum CU QP delta size (determines QG boundaries).
    log2_min_cu_qp_delta_size: u32,
    /// Whether cu_qp_delta_enabled_flag is set in PPS.
    enabled: bool,
    /// Slice QP for CTB row reset.
    slice_qp: i32,
    /// Log2 CTB size for same-CTB neighbor checks.
    ctb_log2: u32,
}

impl QpState {
    fn new(slice_qp: i32, pps: &Pps, sps: &Sps) -> Self {
        let log2_min = if pps._cu_qp_delta_enabled_flag {
            sps.ctb_log2_size()
                .saturating_sub(pps._diff_cu_qp_delta_depth)
        } else {
            sps.ctb_log2_size()
        };
        Self {
            is_cu_qp_delta_coded: false,
            current_qp: slice_qp,
            last_qp_in_previous_qg: slice_qp,
            current_qg_x: -1,
            current_qg_y: -1,
            log2_min_cu_qp_delta_size: log2_min,
            enabled: pps._cu_qp_delta_enabled_flag,
            slice_qp,
            ctb_log2: sps.ctb_log2_size(),
        }
    }

    /// H.265 8.6.1: Reset qPY_PREV to SliceQpY at CTU row boundaries.
    fn reset_for_row(&mut self) {
        self.is_cu_qp_delta_coded = false;
        self.current_qp = self.slice_qp;
        self.last_qp_in_previous_qg = self.slice_qp;
        self.current_qg_x = -1;
        self.current_qg_y = -1;
    }

    /// Enter a new quantization group at CU position (x_cu, y_cu).
    /// If the QG position changes, save current_qp as last_qp_in_previous_qg.
    /// Note: is_cu_qp_delta_coded reset is handled by decode_quadtree (H.265 7.3.8.4).
    fn enter_qg(&mut self, x_cu: u32, y_cu: u32) {
        let qg_mask = (1u32 << self.log2_min_cu_qp_delta_size) - 1;
        let xqg = (x_cu & !qg_mask) as i32;
        let yqg = (y_cu & !qg_mask) as i32;

        if xqg != self.current_qg_x || yqg != self.current_qg_y {
            self.last_qp_in_previous_qg = self.current_qp;
            self.current_qg_x = xqg;
            self.current_qg_y = yqg;
        }
    }

    /// Derive qPY_PRED per H.265 8.6.1 using left/above neighbor QPs.
    /// `pic` provides the stored QPY grid. `x_cu`, `y_cu` is the CU base position.
    fn derive_qp_pred(&self, pic: &Picture, x_cu: u32, y_cu: u32) -> i32 {
        let qg_mask = (1u32 << self.log2_min_cu_qp_delta_size) - 1;
        let xqg = x_cu & !qg_mask;
        let yqg = y_cu & !qg_mask;

        // First QG in CTB row: qPY_PRED = SliceQPY
        let ctb_mask = (1u32 << self.ctb_log2) - 1;
        let first_in_ctb_row = xqg == 0 && (yqg & ctb_mask) == 0;
        if first_in_ctb_row {
            return self.slice_qp;
        }

        let base_pred = self.last_qp_in_previous_qg;

        // Left neighbor QP: QPY at (xQG-1, yQG), only if in same CTB
        let qp_a = if xqg > 0 {
            let left_ctb_x = (xqg - 1) >> self.ctb_log2;
            let cur_ctb_x = xqg >> self.ctb_log2;
            let left_ctb_y = yqg >> self.ctb_log2;
            let cur_ctb_y = yqg >> self.ctb_log2;
            if left_ctb_x == cur_ctb_x && left_ctb_y == cur_ctb_y {
                pic.qp_y_at(xqg - 1, yqg)
            } else {
                base_pred
            }
        } else {
            base_pred
        };

        // Above neighbor QP: QPY at (xQG, yQG-1), only if in same CTB
        let qp_b = if yqg > 0 {
            let above_ctb_x = xqg >> self.ctb_log2;
            let cur_ctb_x = xqg >> self.ctb_log2;
            let above_ctb_y = (yqg - 1) >> self.ctb_log2;
            let cur_ctb_y = yqg >> self.ctb_log2;
            if above_ctb_x == cur_ctb_x && above_ctb_y == cur_ctb_y {
                pic.qp_y_at(xqg, yqg - 1)
            } else {
                base_pred
            }
        } else {
            base_pred
        };

        (qp_a + qp_b + 1) >> 1
    }
}

/// Decode cu_qp_delta_abs and cu_qp_delta_sign_flag (H.265 7.3.8.11, 9.3.3.5).
/// Returns the decoded QP delta value (signed).
fn decode_cu_qp_delta(cab: &mut CabacReader) -> i32 {
    // TU(cMax=5) prefix: up to 5 context-coded bins
    // First bin uses ctx 0, remaining use ctx 1 (matches FFmpeg line 619)
    let b0 = cab.decode_decision(CTX_CU_QP_DELTA);
    if b0 == 0 {
        return 0;
    }

    let mut abs_val = 1u32;
    for _ in 1..5 {
        if cab.decode_decision(CTX_CU_QP_DELTA + 1) == 0 {
            break;
        }
        abs_val += 1;
    }
    // If prefix saturated (all 5 bins = 1), decode EG(k=0) suffix via bypass
    if abs_val == 5 {
        let mut k = 0u32;
        while k < 31 && cab.decode_bypass() != 0 {
            k += 1;
        } // cap to prevent overflow
        if k > 0 {
            abs_val = abs_val.saturating_add((1u32 << k) - 1 + cab.decode_bypass_bits(k));
        }
    }

    // Sign flag (bypass-coded, only if abs > 0)
    let sign = cab.decode_bypass();
    if sign != 0 {
        -(abs_val as i32)
    } else {
        abs_val as i32
    }
}

// Scan type for coefficient scan order (H.265 Section 7.4.9.11)
#[derive(Clone, Copy, PartialEq)]
enum ScanType {
    Diag,
    Horiz,
    Vert,
}

// Diagonal 4x4 scan (Table 6-5) — for coefficients within a 4x4 sub-block
// H.265 Table 6-5: Diagonal scan for 4x4 block.
// Each anti-diagonal traverses from (x=0, y=sum) toward (x=sum, y=0).
// Matches FFmpeg's ff_hevc_diag_scan4x4_x/y.
const DIAG4: [[u8; 2]; 16] = [
    [0, 0],
    [0, 1],
    [1, 0],
    [0, 2],
    [1, 1],
    [2, 0],
    [0, 3],
    [1, 2],
    [2, 1],
    [3, 0],
    [1, 3],
    [2, 2],
    [3, 1],
    [2, 3],
    [3, 2],
    [3, 3],
];

// Horizontal 4x4 scan: raster order (row by row). Matches FFmpeg horiz_scan4x4_x/y.
const HORIZ4: [[u8; 2]; 16] = [
    [0, 0],
    [1, 0],
    [2, 0],
    [3, 0],
    [0, 1],
    [1, 1],
    [2, 1],
    [3, 1],
    [0, 2],
    [1, 2],
    [2, 2],
    [3, 2],
    [0, 3],
    [1, 3],
    [2, 3],
    [3, 3],
];

// Vertical 4x4 scan: column-first order. Matches FFmpeg horiz_scan4x4_y/x.
const VERT4: [[u8; 2]; 16] = [
    [0, 0],
    [0, 1],
    [0, 2],
    [0, 3],
    [1, 0],
    [1, 1],
    [1, 2],
    [1, 3],
    [2, 0],
    [2, 1],
    [2, 2],
    [2, 3],
    [3, 0],
    [3, 1],
    [3, 2],
    [3, 3],
];

// Diagonal sub-block scan tables (Table 6-5 at different sizes)
// 2x2 sub-block grid (for 8x8 TU: 4 sub-blocks)
const DIAG_SUB_2X2: [[u8; 2]; 4] = [[0, 0], [0, 1], [1, 0], [1, 1]];

// Horizontal 2x2 sub-block scan: row-first. Matches FFmpeg horiz_scan2x2_x/y.
const HORIZ_SUB_2X2: [[u8; 2]; 4] = [[0, 0], [1, 0], [0, 1], [1, 1]];

// 4x4 sub-block grid (for 16x16 TU: 16 sub-blocks)
const DIAG_SUB_4X4: [[u8; 2]; 16] = [
    [0, 0],
    [0, 1],
    [1, 0],
    [0, 2],
    [1, 1],
    [2, 0],
    [0, 3],
    [1, 2],
    [2, 1],
    [3, 0],
    [1, 3],
    [2, 2],
    [3, 1],
    [2, 3],
    [3, 2],
    [3, 3],
];

// 8x8 sub-block grid (for 32x32 TU: 64 sub-blocks)
// Generated with correct anti-diagonal direction: x starts at 0, increments.
const DIAG_SUB_8X8: [[u8; 2]; 64] = {
    let mut t = [[0u8; 2]; 64];
    let mut idx = 0;
    let mut diag: u8 = 0;
    while diag < 15 {
        let start_x = if diag < 8 { 0 } else { diag - 7 };
        let start_y = if diag < 8 { diag } else { 7 };
        let mut x = start_x;
        let mut y = start_y;
        loop {
            if x < 8 && y < 8 {
                t[idx] = [x, y];
                idx += 1;
            }
            if y == 0 || x == 7 {
                break;
            }
            x += 1;
            y -= 1;
        }
        diag += 1;
    }
    t
};

/// Remove emulation prevention bytes and return a mapping from coded byte position
/// to RBSP byte position. `map[coded_pos]` = RBSP byte at that coded position.
fn remove_ep_with_map(data: &[u8]) -> (Vec<u8>, Vec<usize>) {
    let mut out = Vec::with_capacity(data.len());
    // map[i] = RBSP byte position corresponding to coded byte position i.
    // Must be indexed by coded position, so use a pre-sized vec, not push.
    let mut map = vec![0usize; data.len() + 1];
    let mut i = 0;
    while i < data.len() {
        map[i] = out.len();
        if i + 2 < data.len() && data[i] == 0x00 && data[i + 1] == 0x00 && data[i + 2] == 0x03 {
            out.push(0x00);
            map[i + 1] = out.len();
            out.push(0x00);
            // The 0x03 byte is removed; map it to the position after the second 0x00
            map[i + 2] = out.len();
            i += 3;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    map[data.len()] = out.len(); // sentinel
    (out, map)
}

/// Decode a single slice from raw coded data (before EP removal).
/// The slice header is parsed with EP-aware BitReader. CABAC data is split
/// into per-substream chunks (for WPP) with EP removal applied per-chunk.
pub fn decode_slice(
    coded: &[u8],
    sps: &Sps,
    pps: &Pps,
    pic: &mut Picture,
    nal_type: u8,
) -> Result<()> {
    // Use EP-aware BitReader for the slice header
    let mut br = BitReader::new(coded);

    // -- Slice header (H.265 Section 7.3.6.1) --

    // 1. first_slice_segment_in_pic_flag
    let first_slice = br.read_flag()?;

    // 2. no_output_of_prior_pics_flag (IRAP NAL types: BLA_W_LP=16..RSV_IRAP_VCL23=23)
    if (16..=23).contains(&nal_type) {
        let _no_output_of_prior_pics_flag = br.read_flag()?;
    }

    // 3. slice_pic_parameter_set_id
    let _slice_pps_id = br.read_ue()?;

    // 4. For non-first slices: dependent_slice_segment_flag and slice_segment_address
    if !first_slice {
        if pps._dependent_slice_segments_enabled_flag {
            let _dependent = br.read_flag()?;
        }
        let pic_size_in_ctbs = sps.pic_width_in_ctbs() * sps.pic_height_in_ctbs();
        let addr_bits = ceil_log2(pic_size_in_ctbs);
        if addr_bits > 0 {
            let _slice_segment_address = br.read_bits(addr_bits as u8)?;
        }
    }

    // 5. num_extra_slice_header_bits (reserved flags, BEFORE slice_type per spec)
    br.skip_bits(pps.num_extra_slice_header_bits as usize)?;

    // 6. slice_type
    let _slice_type = br.read_ue()?;

    // 7. pic_output_flag
    if pps.output_flag_present_flag {
        let _pic_output_flag = br.read_flag()?;
    }

    // 8. For non-IDR: pic_order_cnt_lsb, short_term_ref_pic_set, etc.
    // IDR NAL types (19, 20) skip all POC and reference picture set fields.
    if nal_type != 19 && nal_type != 20 {
        let poc_bits = sps._log2_max_pic_order_cnt_lsb_minus4 + 4;
        let _pic_order_cnt_lsb = br.read_bits(poc_bits as u8)?;

        let st_rps_sps_flag = br.read_flag()?;
        if !st_rps_sps_flag {
            let _ = super::params::parse_short_term_ref_pic_set(
                &mut br,
                sps._num_short_term_ref_pic_sets as usize,
                sps._num_short_term_ref_pic_sets,
                &sps._short_term_ref_pic_sets,
            )?;
        } else if sps._num_short_term_ref_pic_sets > 1 {
            let bits = ceil_log2(sps._num_short_term_ref_pic_sets);
            if bits > 0 {
                let _short_term_ref_pic_set_idx = br.read_bits(bits as u8)?;
            }
        }

        if sps._long_term_ref_pics_present_flag {
            let num_lt_sps = if sps._num_long_term_ref_pics_sps > 0 {
                br.read_ue()?
            } else {
                0
            };
            let num_lt_pics = br.read_ue()?;
            let lt_bits = sps._log2_max_pic_order_cnt_lsb_minus4 + 4;
            for i in 0..(num_lt_sps + num_lt_pics) {
                if i < num_lt_sps {
                    if sps._num_long_term_ref_pics_sps > 1 {
                        let idx_bits = ceil_log2(sps._num_long_term_ref_pics_sps);
                        if idx_bits > 0 {
                            br.read_bits(idx_bits as u8)?;
                        }
                    }
                } else {
                    br.skip_bits(lt_bits as usize)?; // poc_lsb_lt
                    br.skip_bits(1)?; // used_by_curr_pic_lt_flag
                }
                let delta_poc_msb_present = br.read_flag()?;
                if delta_poc_msb_present {
                    let _delta_poc_msb_cycle_lt = br.read_ue()?;
                }
            }
        }

        if sps._sps_temporal_mvp_enabled_flag {
            let _slice_temporal_mvp_enabled = br.read_flag()?;
        }
    }

    // 9. SAO flags (BEFORE slice_qp_delta per spec!)
    let sao_luma = if sps.sample_adaptive_offset_enabled_flag {
        br.read_flag()?
    } else {
        false
    };
    let sao_chroma = if sps.sample_adaptive_offset_enabled_flag {
        br.read_flag()?
    } else {
        false
    };

    // 10. For P/B slices: num_ref_idx_active_override, pred_weight_table, etc.
    // Skip for I-slices (slice_type == 2)

    // 11. slice_qp_delta
    let slice_qp_delta = br.read_se()?;
    let slice_qp = 26 + pps.init_qp_minus26 + slice_qp_delta;

    // 12. Chroma QP offsets
    let mut cb_qp_off = pps.pps_cb_qp_offset;
    let mut cr_qp_off = pps.pps_cr_qp_offset;
    if pps.pps_slice_chroma_qp_offsets_present_flag {
        cb_qp_off += br.read_se()?;
        cr_qp_off += br.read_se()?;
    }

    // 13. Deblocking filter override
    if pps.deblocking_filter_override_enabled_flag && br.read_flag()? {
        let dis = br.read_flag()?;
        if !dis {
            let _ = br.read_se()?; // beta_offset
            let _ = br.read_se()?; // tc_offset
        }
    }

    // 14. slice_loop_filter_across_slices_enabled_flag (H.265 7.3.6.1)
    if pps._loop_filter_across_slices_enabled_flag
        && (sao_luma || sao_chroma || !pps.pps_deblocking_filter_disabled_flag)
    {
        let _slice_loop_filter_across_slices = br.read_flag()?;
    }

    // 15. Entry point offsets (for tiles or WPP)
    let mut entry_point_offsets: Vec<usize> = Vec::new();
    if pps._tiles_enabled_flag || pps._entropy_coding_sync_enabled_flag {
        let num_entry_point_offsets = br.read_ue()?;
        if num_entry_point_offsets > 0 {
            let offset_len_minus1 = br.read_ue()?;
            let offset_bits = (offset_len_minus1 + 1) as u8;
            for _ in 0..num_entry_point_offsets {
                let off = (br.read_bits(offset_bits)? + 1) as usize; // H.265: entry_point_offset_minus1 + 1
                entry_point_offsets.push(off);
            }
        }
    }

    // 16. Slice segment header extension
    if pps._slice_segment_header_extension_present_flag {
        let ext_length = br.read_ue()?;
        for _ in 0..ext_length {
            br.skip_bits(8)?;
        }
    }

    // 15. Byte-align for CABAC
    if br.bits_remaining() > 0 {
        let _ = br.read_bit()?; // alignment_bit_equal_to_one
        while !br.bits_remaining().is_multiple_of(8) {
            let _ = br.read_bit()?;
        }
    }

    // BitReader handles EP removal internally; cabac_start is in CODED bytes
    let header_bits = coded.len() * 8 - br.bits_remaining();
    let cabac_start_coded = header_bits / 8;

    // EP-remove the ENTIRE coded CABAC data, then split into substreams using
    // mapped RBSP byte offsets. This avoids the per-substream EP removal bug where
    // a 00 00 | 03 sequence spanning a substream boundary gets missed.
    let wpp_enabled = pps._entropy_coding_sync_enabled_flag;
    let cabac_coded = &coded[cabac_start_coded..];

    // Build coded→RBSP byte offset map: for each coded byte position,
    // track the corresponding RBSP byte position
    let (rbsp_data, coded_to_rbsp) = remove_ep_with_map(cabac_coded);

    let mut substreams: Vec<&[u8]> = Vec::new();
    if wpp_enabled && !entry_point_offsets.is_empty() {
        // Convert coded byte offsets to RBSP byte offsets
        let mut coded_off = 0usize;
        let mut rbsp_offsets = vec![0usize]; // first substream starts at RBSP byte 0
        for &ep_size in &entry_point_offsets {
            coded_off += ep_size;
            // Map coded offset to RBSP offset
            let rbsp_off = if coded_off < coded_to_rbsp.len() {
                coded_to_rbsp[coded_off]
            } else {
                rbsp_data.len()
            };
            rbsp_offsets.push(rbsp_off);
        }
        rbsp_offsets.push(rbsp_data.len()); // end of last substream

        for i in 0..rbsp_offsets.len() - 1 {
            substreams.push(&rbsp_data[rbsp_offsets[i]..rbsp_offsets[i + 1]]);
        }
    } else {
        substreams.push(&rbsp_data);
    }

    let mut cab = CabacReader::new(substreams[0], 0);
    cab.init_contexts(slice_qp);

    let ctb = sps.ctb_size();
    let w_ctbs = sps.pic_width_in_ctbs();
    let h_ctbs = sps.pic_height_in_ctbs();
    let total = (w_ctbs * h_ctbs) as usize;

    pic.sao_params.resize(total, CtuSaoParams::default());

    let mut qps = QpState::new(slice_qp, pps, sps);
    let mut wpp_saved_ctx: Option<Vec<(u8, u8)>> = None;

    for addr in 0..total {
        let cx = (addr as u32) % w_ctbs;
        let cy = (addr as u32) / w_ctbs;
        let x0 = cx * ctb;
        let y0 = cy * ctb;

        // WPP: At the start of each CTU row (except the first), create a new
        // CabacReader for the next substream and restore CABAC contexts.
        if wpp_enabled && cx == 0 && cy > 0 {
            let substream_idx = cy as usize;
            if substream_idx < substreams.len() {
                let mut new_cab = CabacReader::new(substreams[substream_idx], 0);
                new_cab.init_contexts(slice_qp);
                if let Some(ref saved) = wpp_saved_ctx {
                    new_cab.restore_contexts(saved);
                }
                cab = new_cab;
            }
        }

        // H.265 8.6.1: qPY_PREV = SliceQpY at start of each CTU row
        if cx == 0 {
            qps.reset_for_row();
        }

        if sps.sample_adaptive_offset_enabled_flag {
            pic.sao_params[addr] = decode_sao(
                &mut cab,
                sao_luma,
                sao_chroma,
                cx,
                cy,
                &pic.sao_params,
                addr,
                w_ctbs as usize,
            );
        }

        decode_quadtree(
            &mut cab,
            pic,
            sps,
            pps,
            x0,
            y0,
            ctb,
            sps.ctb_log2_size(),
            0,
            &mut qps,
            cb_qp_off,
            cr_qp_off,
        );

        cabac_trace!(
            "CTU ({},{}) dec={} byp={} trm={}",
            cx,
            cy,
            cab.dec_count,
            cab.byp_count,
            cab.trm_count
        );
        cab.reset_counters();

        // WPP: Save context state after processing CTU column 1 (H.265 9.3.2.3).
        // Note: libde265 also saves at column 1 (ctbx==1 in slice.cc:4804).
        if wpp_enabled && cx == 1 {
            wpp_saved_ctx = Some(cab.save_contexts());
        }

        // H.265 7.3.6.1: end_of_slice_segment_flag via decode_terminate at every CTU.
        let end_of_slice = cab.decode_terminate();
        if end_of_slice {
            break;
        }

        // H.265 7.3.6.1: end_of_sub_stream_one_bit at WPP row boundaries.
        // After end_of_slice_segment_flag==0, if we crossed a row boundary
        // (WPP), decode the substream termination bit.
        if wpp_enabled && cx == w_ctbs - 1 {
            let _end_of_sub_stream = cab.decode_terminate();
        }
    }
    Ok(())
}

/// Decode SAO parameters per H.265 7.3.8.3 sao_coding().
///
/// Correct ordering: for each component (Y, Cb, Cr), decode type then offsets
/// before moving to the next component. Cr type is copied from Cb (not decoded),
/// and Cr eo_class is copied from Cb (not decoded).
#[allow(clippy::too_many_arguments)]
fn decode_sao(
    cab: &mut CabacReader,
    luma: bool,
    chroma: bool,
    rx: u32,
    ry: u32,
    sao_params: &[CtuSaoParams],
    addr: usize,
    w_ctbs: usize,
) -> CtuSaoParams {
    // sao_merge flags are decoded once for the CTU, not per-component
    if rx > 0 && cab.decode_decision(CTX_SAO_MERGE) != 0 {
        return if addr > 0 {
            sao_params[addr - 1].clone()
        } else {
            CtuSaoParams::default()
        };
    }
    if ry > 0 && cab.decode_decision(CTX_SAO_MERGE) != 0 {
        return if addr >= w_ctbs {
            sao_params[addr - w_ctbs].clone()
        } else {
            CtuSaoParams::default()
        };
    }

    let mut p = CtuSaoParams::default();
    let mut chroma_type: u8 = 0;

    // H.265 7.3.8.3: loop cIdx = 0..2, decode type + offsets per component
    for c_idx in 0..3u8 {
        let enabled = (c_idx == 0 && luma) || (c_idx > 0 && chroma);
        if !enabled {
            continue;
        }

        // Decode sao_type_idx: luma gets its own, chroma decoded once at Cb,
        // Cr reuses the Cb type (libde265 line 2786-2797)
        let sao_type = match c_idx {
            0 => decode_sao_type(cab),
            1 => {
                chroma_type = decode_sao_type(cab);
                chroma_type
            }
            _ => chroma_type, // Cr copies type from Cb, not decoded
        };

        if sao_type != 0 {
            let s = decode_sao_component(cab, sao_type, c_idx, &p.cb);
            match c_idx {
                0 => p.y = s,
                1 => p.cb = s,
                _ => p.cr = s,
            }
        }
    }

    p
}

/// Decode sao_type_idx: 0 = not applied, 1 = band offset, 2 = edge offset.
fn decode_sao_type(cab: &mut CabacReader) -> u8 {
    if cab.decode_decision(CTX_SAO_TYPE) == 0 {
        return 0;
    }
    let tb = cab.decode_bypass();
    if tb == 0 { 1 } else { 2 }
}

/// Decode SAO offsets for one component. For Cr (c_idx=2) edge offset,
/// eo_class is copied from Cb rather than decoded (H.265 7.3.8.3).
fn decode_sao_component(
    cab: &mut CabacReader,
    sao_type: u8,
    c_idx: u8,
    cb: &SaoParams,
) -> SaoParams {
    let mut s = SaoParams {
        sao_type_idx: sao_type,
        ..Default::default()
    };

    // sao_offset_abs: truncated unary, max 7
    for i in 0..4 {
        let mut m = 0i8;
        for _ in 0..7 {
            if cab.decode_bypass() == 0 {
                break;
            }
            m += 1;
        }
        s.sao_offset[i] = m;
    }

    if sao_type == 1 {
        // Band offset: signs decoded, then band position
        for i in 0..4 {
            if s.sao_offset[i] != 0 && cab.decode_bypass() != 0 {
                s.sao_offset[i] = -s.sao_offset[i];
            }
        }
        s.sao_band_position = cab.decode_bypass_bits(5) as u8;
    } else {
        // Edge offset: offsets 2,3 are always negative; eo_class decoded
        // for luma and Cb only, Cr copies from Cb
        s.sao_offset[2] = -s.sao_offset[2].abs();
        s.sao_offset[3] = -s.sao_offset[3].abs();
        if c_idx <= 1 {
            s.sao_eo_class = cab.decode_bypass_bits(2) as u8;
        } else {
            s.sao_eo_class = cb.sao_eo_class;
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Quadtree + CU + TU
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn decode_quadtree(
    cab: &mut CabacReader,
    pic: &mut Picture,
    sps: &Sps,
    pps: &Pps,
    x0: u32,
    y0: u32,
    size: u32,
    log2: u32,
    depth: u32,
    qps: &mut QpState,
    cb_qp: i32,
    cr_qp: i32,
) {
    if x0 >= sps.pic_width_in_luma_samples || y0 >= sps.pic_height_in_luma_samples {
        return;
    }

    let split = if log2 > sps.min_cb_log2_size() {
        let forced =
            x0 + size > sps.pic_width_in_luma_samples || y0 + size > sps.pic_height_in_luma_samples;
        if forced {
            true
        } else {
            // FFmpeg ff_hevc_split_coding_unit_flag_decode: context from neighbor CU depths
            let cond_l = if x0 > 0 {
                pic.cu_depth_at(x0 - 1, y0)
                    .map_or(0, |d| if d as u32 > depth { 1 } else { 0 })
            } else {
                0usize
            };
            let cond_a = if y0 > 0 {
                pic.cu_depth_at(x0, y0 - 1)
                    .map_or(0, |d| if d as u32 > depth { 1 } else { 0 })
            } else {
                0usize
            };
            cab.decode_decision(CTX_SPLIT_CU + cond_l + cond_a) != 0
        }
    } else {
        false
    };

    // H.265 7.3.8.4 (libde265 slice.cc:4673): Reset QP delta state at QG boundary.
    // This must happen BEFORE the split decision, at any quadtree node >= QG size.
    if qps.enabled && log2 >= qps.log2_min_cu_qp_delta_size {
        qps.is_cu_qp_delta_coded = false;
    }

    if split {
        let h = size / 2;
        let l = log2 - 1;
        let d = depth + 1;
        decode_quadtree(cab, pic, sps, pps, x0, y0, h, l, d, qps, cb_qp, cr_qp);
        decode_quadtree(cab, pic, sps, pps, x0 + h, y0, h, l, d, qps, cb_qp, cr_qp);
        decode_quadtree(cab, pic, sps, pps, x0, y0 + h, h, l, d, qps, cb_qp, cr_qp);
        decode_quadtree(
            cab,
            pic,
            sps,
            pps,
            x0 + h,
            y0 + h,
            h,
            l,
            d,
            qps,
            cb_qp,
            cr_qp,
        );
    } else {
        decode_cu(
            cab, pic, sps, pps, x0, y0, size, log2, depth, qps, cb_qp, cr_qp,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_cu(
    cab: &mut CabacReader,
    pic: &mut Picture,
    sps: &Sps,
    pps: &Pps,
    x0: u32,
    y0: u32,
    size: u32,
    log2: u32,
    depth: u32,
    qps: &mut QpState,
    cb_qp: i32,
    cr_qp: i32,
) {
    // Store CU depth for neighbor context derivation (CABAC split_cu_flag).
    // This must be set before any neighbor CU reads it for context.
    pic.set_cu_depth(x0, y0, size, depth as u8);

    // H.265: decode_quantization_parameters unconditionally at CU start (libde265 slice.cc:4335).
    // This enters the QG (saving lastQPY if new position) and derives the predicted QP.
    // If no cu_qp_delta is later decoded, QPY = qPY_PRED (delta defaults to 0).
    if qps.enabled {
        qps.enter_qg(x0, y0);
        if !qps.is_cu_qp_delta_coded {
            qps.current_qp = qps.derive_qp_pred(pic, x0, y0);
        }
    }

    // H.265 Section 7.3.8.4, Table 9-34: For I-slices, part_mode is present
    // when log2CbSize == MinCbLog2SizeY. Binarization: bin=1->PART_2Nx2N, bin=0->PART_NxN.
    // H.265 Table 9-34: For I-slices, bin=1->PART_2Nx2N, bin=0->PART_NxN.
    let is_nxn = if size == sps.min_cb_size() {
        cab.decode_decision(CTX_PART_MODE) == 0
    } else {
        false
    };

    if !is_nxn {
        let lm = decode_intra_mode(cab, pic, x0, y0, sps.ctb_log2_size());
        pic.set_intra_mode(x0, y0, size, lm);
        let cm = decode_chroma_mode(cab, lm);
        decode_tt(
            cab, pic, sps, pps, x0, y0, log2, 0, lm, cm, qps, cb_qp, cr_qp, true, true,
        );
    } else {
        // H.265 Section 7.3.8.5: PART_NxN — all prev_intra_luma_pred_flag bins
        // FIRST, then all rem/mpm modes, then ONE chroma mode (for 4:2:0).
        let h = size / 2;
        let pos = [(x0, y0), (x0 + h, y0), (x0, y0 + h), (x0 + h, y0 + h)];
        let mut lm = [0u8; 4];

        // Step 1: Decode all 4 prev_intra_luma_pred_flag bins (context-coded)
        let mut prev_flag = [false; 4];
        for flag in &mut prev_flag {
            *flag = cab.decode_decision(CTX_PREV_INTRA_PRED) != 0;
        }

        // Step 2: Decode all 4 rem/mpm modes (bypass-coded)
        for i in 0..4 {
            let (px, py) = pos[i];
            let mpm = derive_mpm(pic, px, py, sps.ctb_log2_size());
            lm[i] = if prev_flag[i] {
                let idx = if cab.decode_bypass() == 0 {
                    0
                } else if cab.decode_bypass() == 0 {
                    1
                } else {
                    2
                };
                mpm[idx]
            } else {
                let rem = cab.decode_bypass_bits(5) as u8;
                let mut sorted = mpm;
                sorted.sort();
                let mut m = rem;
                for &mp in &sorted {
                    if m >= mp {
                        m += 1;
                    }
                }
                m.min(34)
            };
            pic.set_intra_mode(px, py, h, lm[i]);
        }

        // Step 3: Decode ONE chroma mode (H.265: for chroma_format_idc != 3)
        let cm_single = decode_chroma_mode(cab, lm[0]);
        let cm = [cm_single; 4];
        // cbf_cb/cbf_cr decoded at CU level (H.265 7.3.8.7)
        let cbf_cb = if log2 > 2 {
            cab.decode_decision(CTX_CBF_CHROMA) != 0
        } else {
            false
        };
        let cbf_cr = if log2 > 2 {
            cab.decode_decision(CTX_CBF_CHROMA) != 0
        } else {
            false
        };
        // IntraSplitFlag forces split — no split_transform_flag decoded
        for i in 0..4 {
            let (px, py) = pos[i];
            if px >= sps.pic_width_in_luma_samples || py >= sps.pic_height_in_luma_samples {
                continue;
            }
            decode_tu_nxn(
                cab,
                pic,
                sps,
                pps,
                px,
                py,
                log2 - 1,
                lm[i],
                cm[i],
                qps,
                cb_qp,
                cr_qp,
                cbf_cb,
                cbf_cr,
                x0,
                y0,
                i as u8,
            );
        }
    }

    // Store per-CU QP for deblocking filter (H.265 8.7.2.4)
    pic.set_qp_y(x0, y0, size, qps.current_qp);
    cabac_trace!(
        "CU_QP xCU={} yCU={} size={} QPY={}",
        x0,
        y0,
        size,
        qps.current_qp
    );
}

/// Derive Most Probable Modes from left/above neighbors (H.265 8.4.2).
/// The above neighbor is unavailable if it lies in a different CTU row.
fn derive_mpm(pic: &Picture, x0: u32, y0: u32, ctb_log2: u32) -> [u8; 3] {
    let cand_a = if x0 > 0 {
        pic.intra_mode_at(x0 - 1, y0)
    } else {
        1
    };
    let cand_b = if y0 > 0 && (y0 >> ctb_log2) == ((y0 - 1) >> ctb_log2) {
        pic.intra_mode_at(x0, y0 - 1)
    } else {
        1
    };

    if cand_a == cand_b {
        if cand_a < 2 {
            [0, 1, 26]
        } else {
            let a = cand_a as u32;
            [
                cand_a,
                2 + ((a - 2 + 31) & 31) as u8,
                2 + ((a - 2 + 1) & 31) as u8,
            ]
        }
    } else {
        let (a, b) = (cand_a, cand_b);
        let third = if a != 0 && b != 0 {
            0
        } else if a != 1 && b != 1 {
            1
        } else {
            26
        };
        [a, b, third]
    }
}

fn decode_intra_mode(cab: &mut CabacReader, pic: &Picture, x0: u32, y0: u32, ctb_log2: u32) -> u8 {
    let mpm = derive_mpm(pic, x0, y0, ctb_log2);
    if cab.decode_decision(CTX_PREV_INTRA_PRED) != 0 {
        let idx = if cab.decode_bypass() == 0 {
            0
        } else if cab.decode_bypass() == 0 {
            1
        } else {
            2
        };
        mpm[idx]
    } else {
        let rem = cab.decode_bypass_bits(5) as u8;
        let mut sorted = mpm;
        sorted.sort();
        let mut m = rem;
        for &mp in &sorted {
            if m >= mp {
                m += 1;
            }
        }
        m.min(34)
    }
}

fn decode_chroma_mode(cab: &mut CabacReader, luma: u8) -> u8 {
    if cab.decode_decision(CTX_CHROMA_PRED) == 0 {
        luma
    } else {
        let table = [0u8, 26, 10, 1];
        let mapped = table[cab.decode_bypass_bits(2) as usize & 3];
        // H.265 8.4.3: if mapped mode == luma mode, substitute mode 34
        if mapped == luma { 34 } else { mapped }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_tt(
    cab: &mut CabacReader,
    pic: &mut Picture,
    sps: &Sps,
    pps: &Pps,
    x0: u32,
    y0: u32,
    log2: u32,
    depth: u32,
    lm: u8,
    cm: u8,
    qps: &mut QpState,
    cb_qp: i32,
    cr_qp: i32,
    parent_cbf_cb: bool,
    parent_cbf_cr: bool,
) {
    if x0 >= sps.pic_width_in_luma_samples || y0 >= sps.pic_height_in_luma_samples {
        return;
    }
    // H.265 Section 7.3.8.7: cbf_cb/cbf_cr are read at the TOP of transform_tree,
    // BEFORE the split decision, when log2TrafoSize > 2 and parent cbf allows it.
    let cbf_cb = if log2 > 2 && (depth == 0 || parent_cbf_cb) {
        cab.decode_decision(CTX_CBF_CHROMA + depth.min(3) as usize) != 0
    } else if log2 <= 2 {
        parent_cbf_cb
    } else {
        false
    };
    let cbf_cr = if log2 > 2 && (depth == 0 || parent_cbf_cr) {
        cab.decode_decision(CTX_CBF_CHROMA + depth.min(3) as usize) != 0
    } else if log2 <= 2 {
        parent_cbf_cr
    } else {
        false
    };

    let split = if log2 > sps.max_tb_log2_size() {
        true
    } else if log2 <= sps.min_tb_log2_size() || depth >= sps.max_transform_hierarchy_depth_intra {
        false
    } else {
        cab.decode_decision(CTX_SPLIT_TF + (5u32.saturating_sub(log2)).min(2) as usize) != 0
    };

    if split {
        let h = 1u32 << (log2 - 1);
        let d = depth + 1;
        let l = log2 - 1;
        decode_tt(
            cab, pic, sps, pps, x0, y0, l, d, lm, cm, qps, cb_qp, cr_qp, cbf_cb, cbf_cr,
        );
        decode_tt(
            cab,
            pic,
            sps,
            pps,
            x0 + h,
            y0,
            l,
            d,
            lm,
            cm,
            qps,
            cb_qp,
            cr_qp,
            cbf_cb,
            cbf_cr,
        );
        decode_tt(
            cab,
            pic,
            sps,
            pps,
            x0,
            y0 + h,
            l,
            d,
            lm,
            cm,
            qps,
            cb_qp,
            cr_qp,
            cbf_cb,
            cbf_cr,
        );
        decode_tt(
            cab,
            pic,
            sps,
            pps,
            x0 + h,
            y0 + h,
            l,
            d,
            lm,
            cm,
            qps,
            cb_qp,
            cr_qp,
            cbf_cb,
            cbf_cr,
        );
    } else {
        decode_tu(
            cab, pic, sps, pps, x0, y0, log2, depth, lm, cm, qps, cb_qp, cr_qp, cbf_cb, cbf_cr,
        );
    }
}

/// Decode a TU for NxN sub-partition. Only decodes luma. Chroma is decoded
/// at blk_idx==3 covering the full CU chroma block.
#[allow(clippy::too_many_arguments)]
fn decode_tu_nxn(
    cab: &mut CabacReader,
    pic: &mut Picture,
    sps: &Sps,
    pps: &Pps,
    x0: u32,
    y0: u32,
    log2: u32,
    lm: u8,
    cm: u8,
    qps: &mut QpState,
    cb_qp: i32,
    cr_qp: i32,
    cbf_cb: bool,
    cbf_cr: bool,
    x_base: u32,
    y_base: u32,
    blk_idx: u8,
) {
    let size = 1u32 << log2;
    let bd = pic.bit_depth;
    let max_val = (1i32 << bd) - 1;

    // FFmpeg: cbf_luma context = !trafo_depth (line 833). Always decoded for INTRA.
    // NxN: trafo_depth=1 (IntraSplitFlag forced split), so context = !(1) = 0 → CTX_CBF_LUMA+0
    let cbf_y = cab.decode_decision(CTX_CBF_LUMA) != 0;

    // H.265 7.3.8.11: cu_qp_delta decoded in first TU with non-zero cbf in a QG
    if qps.enabled && !qps.is_cu_qp_delta_coded && (cbf_y || cbf_cb || cbf_cr) {
        let delta = decode_cu_qp_delta(cab);
        // H.265 8.6.1: QPY = (qPY_PRED + CuQpDelta + 52) % 52
        let qp_pred = qps.derive_qp_pred(pic, x_base, y_base);
        let bd_offset = 6 * (pic.bit_depth as i32 - 8);
        qps.current_qp = ((qp_pred + delta + 52 + 2 * bd_offset) % (52 + bd_offset)) - bd_offset;
        qps.is_cu_qp_delta_coded = true;
    }

    let qp = qps.current_qp;
    let sign_data_hiding = pps.sign_data_hiding_enabled_flag;

    // Derive scan types from luma transform log2 (FFmpeg: scan_idx depends on luma TU size)
    let scan_luma = derive_scan_type(log2, lm);
    let scan_chroma = derive_scan_type(log2, cm);

    // H.265 7.3.8.11: transform_skip_flag decoded per component before residual_coding
    let ts_y = if pps.transform_skip_enabled_flag && log2 <= 2 {
        cab.decode_decision(CTX_TRANSFORM_SKIP) != 0
    } else {
        false
    };

    // Luma
    let pred = predict_intra(
        pic,
        x0,
        y0,
        size,
        lm,
        Component::Y,
        sps.strong_intra_smoothing_enabled_flag,
    );
    if cbf_y {
        let mut c = vec![0i32; (size * size) as usize];
        decode_residual(cab, &mut c, log2, 0, sign_data_hiding, scan_luma);
        transform::dequantize(&mut c, qp, bd, log2, sps.scaling_list_enabled_flag, 0);
        if ts_y {
            transform::transform_skip(&mut c, size, bd);
        } else {
            transform::inverse_transform(&mut c, size, size == 4, bd);
        }
        for py in 0..size {
            for px in 0..size {
                let sx = x0 + px;
                let sy = y0 + py;
                if sx < pic.width && sy < pic.height {
                    let i = (py * size + px) as usize;
                    pic.set_y(sx, sy, (pred[i] as i32 + c[i]).clamp(0, max_val) as i16);
                }
            }
        }
        pic.mark_reconstructed(x0, y0, size);
    } else {
        for py in 0..size {
            for px in 0..size {
                let sx = x0 + px;
                let sy = y0 + py;
                if sx < pic.width && sy < pic.height {
                    pic.set_y(sx, sy, pred[(py * size + px) as usize]);
                }
            }
        }
        pic.mark_reconstructed(x0, y0, size);
    }

    // H.265 7.3.8.11: chroma decoded only at blkIdx==3 for 4x4 sub-TUs
    if blk_idx != 3 {
        return;
    }

    // Chroma uses the enclosing CU coordinates and log2+1 size
    let cu_size = size * 2; // 8x8 CU
    let cu_log2 = log2 + 1;
    let cs = cu_size / 2; // 4x4 chroma
    let cl = cu_log2 - 1; // log2(4) = 2
    let cx = x_base / 2;
    let cy = y_base / 2;
    let cw = pic.width.div_ceil(2);
    let ch = pic.height.div_ceil(2);
    let cb_qp_actual = chroma_qp(qp + cb_qp);
    let cr_qp_actual = chroma_qp(qp + cr_qp);

    // H.265 7.3.8.11: chroma transform_skip_flag (context +1 for chroma)
    let ts_cb = if pps.transform_skip_enabled_flag && cl <= 2 {
        cab.decode_decision(CTX_TRANSFORM_SKIP + 1) != 0
    } else {
        false
    };

    let pred_cb = predict_intra(pic, x_base, y_base, cu_size, cm, Component::Cb, false);
    if cbf_cb {
        let mut c = vec![0i32; (cs * cs) as usize];
        decode_residual(cab, &mut c, cl, 1, sign_data_hiding, scan_chroma);
        transform::dequantize(
            &mut c,
            cb_qp_actual,
            bd,
            cl,
            sps.scaling_list_enabled_flag,
            1,
        );
        if ts_cb {
            transform::transform_skip(&mut c, cs, bd);
        } else {
            transform::inverse_transform(&mut c, cs, false, bd);
        }
        for py in 0..cs {
            for px in 0..cs {
                let sx = cx + px;
                let sy = cy + py;
                if sx < cw && sy < ch {
                    let i = (py * cs + px) as usize;
                    pic.set_cb(sx, sy, (pred_cb[i] as i32 + c[i]).clamp(0, max_val) as i16);
                }
            }
        }
    } else {
        for py in 0..cs {
            for px in 0..cs {
                let sx = cx + px;
                let sy = cy + py;
                if sx < cw && sy < ch {
                    pic.set_cb(sx, sy, pred_cb[(py * cs + px) as usize]);
                }
            }
        }
    }

    let ts_cr = if pps.transform_skip_enabled_flag && cl <= 2 {
        cab.decode_decision(CTX_TRANSFORM_SKIP + 1) != 0
    } else {
        false
    };

    let pred_cr = predict_intra(pic, x_base, y_base, cu_size, cm, Component::Cr, false);
    if cbf_cr {
        let mut c = vec![0i32; (cs * cs) as usize];
        decode_residual(cab, &mut c, cl, 2, sign_data_hiding, scan_chroma);
        transform::dequantize(
            &mut c,
            cr_qp_actual,
            bd,
            cl,
            sps.scaling_list_enabled_flag,
            2,
        );
        if ts_cr {
            transform::transform_skip(&mut c, cs, bd);
        } else {
            transform::inverse_transform(&mut c, cs, false, bd);
        }
        for py in 0..cs {
            for px in 0..cs {
                let sx = cx + px;
                let sy = cy + py;
                if sx < cw && sy < ch {
                    let i = (py * cs + px) as usize;
                    pic.set_cr(sx, sy, (pred_cr[i] as i32 + c[i]).clamp(0, max_val) as i16);
                }
            }
        }
    } else {
        for py in 0..cs {
            for px in 0..cs {
                let sx = cx + px;
                let sy = cy + py;
                if sx < cw && sy < ch {
                    pic.set_cr(sx, sy, pred_cr[(py * cs + px) as usize]);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_tu(
    cab: &mut CabacReader,
    pic: &mut Picture,
    sps: &Sps,
    pps: &Pps,
    x0: u32,
    y0: u32,
    log2: u32,
    depth: u32,
    lm: u8,
    cm: u8,
    qps: &mut QpState,
    cb_qp: i32,
    cr_qp: i32,
    inherited_cbf_cb: bool,
    inherited_cbf_cr: bool,
) {
    let size = 1u32 << log2;
    let bd = pic.bit_depth;
    let max_val = (1i32 << bd) - 1;

    let cbf_cb = inherited_cbf_cb;
    let cbf_cr = inherited_cbf_cr;
    // FFmpeg: cbf_luma always decoded for INTRA; context = !trafo_depth (line 833)
    let cbf_y = cab.decode_decision(CTX_CBF_LUMA + (depth == 0) as usize) != 0;

    // H.265 7.3.8.11: cu_qp_delta decoded in first TU with non-zero cbf in a QG
    if qps.enabled && !qps.is_cu_qp_delta_coded && (cbf_y || cbf_cb || cbf_cr) {
        let delta = decode_cu_qp_delta(cab);
        // H.265 8.6.1: QPY = (qPY_PRED + CuQpDelta + 52) % 52
        let qp_pred = qps.derive_qp_pred(pic, x0, y0);
        let bd_offset = 6 * (pic.bit_depth as i32 - 8);
        qps.current_qp = ((qp_pred + delta + 52 + 2 * bd_offset) % (52 + bd_offset)) - bd_offset;
        qps.is_cu_qp_delta_coded = true;
    }

    let qp = qps.current_qp;
    // Chroma QP = clamp(luma_qp + offset, 0, 51) then map through QpC table
    let cb_qp_actual = chroma_qp(qp + cb_qp);
    let cr_qp_actual = chroma_qp(qp + cr_qp);

    let sign_data_hiding = pps.sign_data_hiding_enabled_flag;

    // Derive scan types from luma TU log2 (FFmpeg: both scan_idx and scan_idx_c use luma TU size)
    let scan_luma = derive_scan_type(log2, lm);
    let scan_chroma = derive_scan_type(log2, cm);

    // H.265 7.3.8.11: transform_skip_flag decoded per component before residual_coding
    let ts_y = if pps.transform_skip_enabled_flag && log2 <= 2 {
        cab.decode_decision(CTX_TRANSFORM_SKIP) != 0
    } else {
        false
    };

    // Luma
    let pred = predict_intra(
        pic,
        x0,
        y0,
        size,
        lm,
        Component::Y,
        sps.strong_intra_smoothing_enabled_flag,
    );
    if cbf_y {
        let mut c = vec![0i32; (size * size) as usize];
        decode_residual(cab, &mut c, log2, 0, sign_data_hiding, scan_luma);
        transform::dequantize(&mut c, qp, bd, log2, sps.scaling_list_enabled_flag, 0);
        if ts_y {
            transform::transform_skip(&mut c, size, bd);
        } else {
            transform::inverse_transform(&mut c, size, size == 4, bd);
        }
        for py in 0..size {
            for px in 0..size {
                let sx = x0 + px;
                let sy = y0 + py;
                if sx < pic.width && sy < pic.height {
                    let i = (py * size + px) as usize;
                    pic.set_y(sx, sy, (pred[i] as i32 + c[i]).clamp(0, max_val) as i16);
                }
            }
        }
        pic.mark_reconstructed(x0, y0, size);
    } else {
        for py in 0..size {
            for px in 0..size {
                let sx = x0 + px;
                let sy = y0 + py;
                if sx < pic.width && sy < pic.height {
                    pic.set_y(sx, sy, pred[(py * size + px) as usize]);
                }
            }
        }
        pic.mark_reconstructed(x0, y0, size);
    }

    if log2 <= 2 {
        return;
    }
    let cs = size / 2;
    let cl = log2 - 1;
    let cx = x0 / 2;
    let cy = y0 / 2;
    let cw = pic.width.div_ceil(2);
    let ch = pic.height.div_ceil(2);

    // H.265 7.3.8.11: chroma transform_skip_flag (context +1 for chroma)
    let ts_cb = if pps.transform_skip_enabled_flag && cl <= 2 {
        cab.decode_decision(CTX_TRANSFORM_SKIP + 1) != 0
    } else {
        false
    };

    // Cb
    let pred_cb = predict_intra(pic, x0, y0, size, cm, Component::Cb, false);
    if cbf_cb {
        let mut c = vec![0i32; (cs * cs) as usize];
        decode_residual(cab, &mut c, cl, 1, sign_data_hiding, scan_chroma);
        transform::dequantize(
            &mut c,
            cb_qp_actual,
            bd,
            cl,
            sps.scaling_list_enabled_flag,
            1,
        );
        if ts_cb {
            transform::transform_skip(&mut c, cs, bd);
        } else {
            transform::inverse_transform(&mut c, cs, false, bd);
        }
        for py in 0..cs {
            for px in 0..cs {
                let sx = cx + px;
                let sy = cy + py;
                if sx < cw && sy < ch {
                    let i = (py * cs + px) as usize;
                    pic.set_cb(sx, sy, (pred_cb[i] as i32 + c[i]).clamp(0, max_val) as i16);
                }
            }
        }
    } else {
        for py in 0..cs {
            for px in 0..cs {
                let sx = cx + px;
                let sy = cy + py;
                if sx < cw && sy < ch {
                    pic.set_cb(sx, sy, pred_cb[(py * cs + px) as usize]);
                }
            }
        }
    }

    let ts_cr = if pps.transform_skip_enabled_flag && cl <= 2 {
        cab.decode_decision(CTX_TRANSFORM_SKIP + 1) != 0
    } else {
        false
    };

    // Cr
    let pred_cr = predict_intra(pic, x0, y0, size, cm, Component::Cr, false);
    if cbf_cr {
        let mut c = vec![0i32; (cs * cs) as usize];
        decode_residual(cab, &mut c, cl, 2, sign_data_hiding, scan_chroma);
        transform::dequantize(
            &mut c,
            cr_qp_actual,
            bd,
            cl,
            sps.scaling_list_enabled_flag,
            2,
        );
        if ts_cr {
            transform::transform_skip(&mut c, cs, bd);
        } else {
            transform::inverse_transform(&mut c, cs, false, bd);
        }
        for py in 0..cs {
            for px in 0..cs {
                let sx = cx + px;
                let sy = cy + py;
                if sx < cw && sy < ch {
                    let i = (py * cs + px) as usize;
                    pic.set_cr(sx, sy, (pred_cr[i] as i32 + c[i]).clamp(0, max_val) as i16);
                }
            }
        }
    } else {
        for py in 0..cs {
            for px in 0..cs {
                let sx = cx + px;
                let sy = cy + py;
                if sx < cw && sy < ch {
                    pic.set_cr(sx, sy, pred_cr[(py * cs + px) as usize]);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Residual coding
// ---------------------------------------------------------------------------

fn derive_scan_type(luma_log2_trafo: u32, intra_pred_mode: u8) -> ScanType {
    if luma_log2_trafo < 4 {
        if (6..=14).contains(&intra_pred_mode) {
            ScanType::Vert
        } else if (22..=30).contains(&intra_pred_mode) {
            ScanType::Horiz
        } else {
            ScanType::Diag
        }
    } else {
        ScanType::Diag
    }
}

fn decode_residual(
    cab: &mut CabacReader,
    coeffs: &mut [i32],
    log2: u32,
    c_idx: u8,
    sign_data_hiding: bool,
    scan_type: ScanType,
) {
    // FFmpeg sig_coeff_flag context index maps (Table 9-39 derivation)
    #[rustfmt::skip]
    const CTX_IDX_MAP: [[u8; 16]; 5] = [
        [0, 1, 4, 5, 2, 3, 4, 5, 6, 6, 8, 8, 7, 7, 8, 8], // log2_trafo_size == 2
        [1, 1, 1, 0, 1, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0], // prev_sig == 0
        [2, 2, 2, 2, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0], // prev_sig == 1
        [2, 1, 0, 0, 2, 1, 0, 0, 2, 1, 0, 0, 2, 1, 0, 0], // prev_sig == 2
        [2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2], // prev_sig >= 3
    ];

    let size = 1u32 << log2;

    // Coefficient scan table based on scan type
    let coeff_scan: &[[u8; 2]; 16] = match scan_type {
        ScanType::Diag => &DIAG4,
        ScanType::Horiz => &HORIZ4,
        ScanType::Vert => &VERT4,
    };

    let (ctx_off, ctx_shift) = if c_idx == 0 {
        (
            3 * (log2 as usize - 2) + ((log2 as usize - 1) >> 2),
            (log2 + 1) >> 2,
        )
    } else {
        (15usize, log2 - 2)
    };

    // FFmpeg order: x prefix, y prefix, THEN x suffix, y suffix
    // (all context-coded bins before any bypass-coded bins)
    let pfx_x = decode_last_prefix(cab, CTX_LAST_X + ctx_off, log2, ctx_shift);
    let pfx_y = decode_last_prefix(cab, CTX_LAST_Y + ctx_off, log2, ctx_shift);
    let (last_x, last_y) = {
        let lx = decode_last_suffix(cab, pfx_x);
        let ly = decode_last_suffix(cab, pfx_y);
        // H.265 / FFmpeg line 1129-1130: swap x/y for vertical scan
        if scan_type == ScanType::Vert {
            (ly, lx)
        } else {
            (lx, ly)
        }
    };

    let spr = size / 4;

    // Sub-block scan table based on scan type
    let sub_scan: &[[u8; 2]] = match spr {
        1 => &[[0, 0]][..],
        2 => match scan_type {
            ScanType::Horiz => &HORIZ_SUB_2X2,
            _ => &DIAG_SUB_2X2,
        },
        4 => &DIAG_SUB_4X4,
        8 => &DIAG_SUB_8X8,
        _ => return,
    };

    // Find last sub-block scan index
    let last_sbx = last_x / 4;
    let last_sby = last_y / 4;
    let last_sub = sub_scan
        .iter()
        .position(|&[sx, sy]| sx as u32 == last_sbx && sy as u32 == last_sby)
        .unwrap_or(0) as u32;

    // Track coded sub-block flags for neighbor context derivation
    let mut coded_sb = vec![vec![false; spr as usize]; spr as usize];
    // Track gt1 state across sub-blocks (FFmpeg: greater1_ctx persistence)
    let mut prev_sub_gt1 = false;

    for sub_idx in (0..=last_sub).rev() {
        let [sbx, sby] = sub_scan[sub_idx as usize];
        let sx = sbx as u32 * 4;
        let sy = sby as u32 * 4;

        // coded_sub_block_flag with neighbor-based context (FFmpeg lines 1194-1208)
        let coded = if sub_idx == last_sub || sub_idx == 0 {
            true
        } else {
            let right_coded = if (sbx as u32 + 1) < spr {
                coded_sb[sby as usize][(sbx + 1) as usize] as u32
            } else {
                0
            };
            let below_coded = if (sby as u32 + 1) < spr {
                coded_sb[(sby + 1) as usize][sbx as usize] as u32
            } else {
                0
            };
            let ctx_cg = right_coded + below_coded;
            let inc = ctx_cg.min(1) as usize + if c_idx > 0 { 2 } else { 0 };
            cab.decode_decision(CTX_CODED_SUB + inc) != 0
        };

        coded_sb[sby as usize][sbx as usize] = coded;

        if !coded {
            continue;
        }

        let mut sig = [false; 16];

        let mut infer_dc = sub_idx > 0 && sub_idx < last_sub && coded;

        let last_scan_idx = if sub_idx == last_sub {
            if last_x >= sx && last_y >= sy {
                let lx = (last_x - sx) as u8;
                let ly = (last_y - sy) as u8;
                coeff_scan
                    .iter()
                    .position(|p| p[0] == lx && p[1] == ly)
                    .unwrap_or(15)
            } else {
                15
            }
        } else {
            16
        };

        // Compute prev_sig from neighbor coded sub-block flags (FFmpeg lines 1220-1223)
        let prev_sig = {
            let mut ps = 0u32;
            if (sbx as u32) < ((1u32 << log2) - 1) >> 2 {
                ps = coded_sb[sby as usize][(sbx + 1) as usize] as u32;
            }
            if (sby as u32) < ((1u32 << log2) - 1) >> 2 {
                ps += (coded_sb[(sby + 1) as usize][sbx as usize] as u32) << 1;
            }
            ps
        };

        for sp in (0..16).rev() {
            let px = coeff_scan[sp][0] as u32 + sx;
            let py = coeff_scan[sp][1] as u32 + sy;
            if px >= size || py >= size {
                continue;
            }
            if px == last_x && py == last_y && sub_idx == last_sub {
                sig[sp] = true;
                infer_dc = false;
                continue;
            }
            if sub_idx == last_sub && sp > last_scan_idx {
                continue;
            }
            if infer_dc && sp == 0 {
                sig[sp] = true;
                continue;
            }

            // sig_coeff_flag context (FFmpeg lines 1226-1300)
            let ctx = if sp > 0 {
                // Non-DC coefficient
                let mut scf_offset;
                let map_idx;
                if c_idx != 0 {
                    scf_offset = 27usize;
                } else {
                    scf_offset = 0;
                }
                if log2 == 2 {
                    map_idx = 0;
                } else {
                    map_idx = (prev_sig as usize + 1).min(4);
                    if c_idx == 0 {
                        if sbx > 0 || sby > 0 {
                            scf_offset += 3;
                        }
                        // FFmpeg: diagonal scan uses offset 9, non-diagonal uses 15
                        if log2 == 3 {
                            scf_offset += if scan_type == ScanType::Diag { 9 } else { 15 };
                        } else {
                            scf_offset += 21;
                        }
                    } else if log2 == 3 {
                        scf_offset += 9;
                    } else {
                        scf_offset += 12;
                    }
                }
                let x_c = coeff_scan[sp][0];
                let y_c = coeff_scan[sp][1];
                scf_offset + CTX_IDX_MAP[map_idx][(y_c as usize) * 4 + x_c as usize] as usize
            } else {
                // DC coefficient (sp == 0, not inferred)
                if sub_idx == 0 {
                    if c_idx == 0 { 0 } else { 27 }
                } else {
                    // Non-DC sub-block DC position: use scf_offset + 2
                    let mut scf_offset;
                    if c_idx != 0 {
                        scf_offset = 27usize;
                    } else {
                        scf_offset = 0;
                    }
                    if log2 != 2 {
                        if c_idx == 0 {
                            if sbx > 0 || sby > 0 {
                                scf_offset += 3;
                            }
                            if log2 == 3 {
                                scf_offset += if scan_type == ScanType::Diag { 9 } else { 15 };
                            } else {
                                scf_offset += 21;
                            }
                        } else if log2 == 3 {
                            scf_offset += 9;
                        } else {
                            scf_offset += 12;
                        }
                    }
                    2 + scf_offset
                }
            };

            if cab.decode_decision(CTX_SIG_COEFF + ctx) != 0 {
                sig[sp] = true;
                infer_dc = false;
            }
        }

        let mut abs = [0i32; 16];
        let mut gt1_cnt = 0u32;
        let mut first_gt2: Option<usize> = None;
        let mut first_gt1 = true;
        let mut had_gt1 = [false; 16];

        // gt1/gt2 context (FFmpeg lines 1319-1358, 924-938)
        let mut ctx_set = if sub_idx > 0 && c_idx == 0 { 2usize } else { 0 };
        if sub_idx < last_sub && prev_sub_gt1 {
            ctx_set += 1;
        }
        let chroma_off = if c_idx > 0 { 16 } else { 0 };
        let gt2_chroma = if c_idx > 0 { 4 } else { 0 };

        // Reset gt1_ctx for this sub-block (FFmpeg: greater1_ctx = 1)
        let mut gt1_ctx = 1usize;

        for sp in (0..16).rev() {
            if !sig[sp] {
                continue;
            }
            abs[sp] = 1;
            if gt1_cnt < 8 {
                had_gt1[sp] = true;
                if cab.decode_decision(CTX_GT1 + (ctx_set << 2) + gt1_ctx + chroma_off) != 0 {
                    abs[sp] = 2;
                    if first_gt1 {
                        first_gt2 = Some(sp);
                    }
                    gt1_ctx = 0;
                    first_gt1 = false;
                } else if first_gt1 {
                    gt1_ctx = (gt1_ctx + 1).min(3);
                }
                gt1_cnt += 1;
            }
        }

        // Track whether this sub-block had any gt1 (for next sub-block's ctx_set)
        prev_sub_gt1 = gt1_ctx == 0;

        if let Some(p) = first_gt2
            && cab.decode_decision(CTX_GT2 + ctx_set + gt2_chroma) != 0
        {
            abs[p] = 3;
        }

        // Sign data hiding (H.265 7.4.9.11): determine if sign of first
        // non-zero coefficient is inferred from parity rather than decoded.
        let mut first_nz_pos: i32 = 16;
        let mut last_nz_pos: i32 = -1;
        for (sp, &is_sig) in sig.iter().enumerate() {
            if is_sig {
                if first_nz_pos == 16 {
                    first_nz_pos = sp as i32;
                }
                last_nz_pos = sp as i32;
            }
        }
        let sign_hidden = sign_data_hiding && (last_nz_pos - first_nz_pos >= 4);

        let mut signs = [false; 16];
        for sp in (0..16).rev() {
            if !sig[sp] {
                continue;
            }
            // Skip sign decode for first_nz_pos when sign is hidden
            if sign_hidden && sp == first_nz_pos as usize {
                continue;
            }
            signs[sp] = cab.decode_bypass() != 0;
        }

        let mut c_rice_param = 0u32;
        let mut sum_abs = 0i32;
        for sp in (0..16).rev() {
            if !sig[sp] {
                continue;
            }
            let need_remaining = if had_gt1[sp] {
                if Some(sp) == first_gt2 {
                    abs[sp] >= 3
                } else {
                    abs[sp] >= 2
                }
            } else {
                true
            };
            if need_remaining {
                abs[sp] += decode_remaining(cab, c_rice_param) as i32;
            }
            let abs_level = abs[sp] as u32;
            if abs_level > (3 << c_rice_param) && c_rice_param < 4 {
                c_rice_param += 1;
            }
            sum_abs = sum_abs.wrapping_add(abs[sp]);
            // Sign: inferred from parity for first_nz_pos, decoded for others
            let c = if sign_hidden && sp == first_nz_pos as usize {
                if (sum_abs & 1) != 0 {
                    -abs[sp]
                } else {
                    abs[sp]
                }
            } else if signs[sp] {
                -abs[sp]
            } else {
                abs[sp]
            };
            let px = coeff_scan[sp][0] as u32 + sx;
            let py = coeff_scan[sp][1] as u32 + sy;
            if px < size && py < size {
                coeffs[(py * size + px) as usize] = c;
            }
        }
    }
}

/// Decode last_sig_coeff PREFIX only (context-coded bins).
fn decode_last_prefix(cab: &mut CabacReader, ctx_base: usize, log2: u32, ctx_shift: u32) -> u32 {
    let max_p = ((log2 << 1) - 1).min(9);
    let mut pfx = 0u32;
    for i in 0..max_p {
        if cab.decode_decision(ctx_base + (i >> ctx_shift) as usize) == 0 {
            break;
        }
        pfx += 1;
    }
    pfx
}

/// Decode last_sig_coeff SUFFIX (bypass-coded bins) from a previously decoded prefix.
fn decode_last_suffix(cab: &mut CabacReader, pfx: u32) -> u32 {
    if pfx < 4 {
        pfx
    } else {
        let sl = (pfx >> 1) - 1;
        let sfx = cab.decode_bypass_bits(sl);
        ((2 + (pfx & 1)) << sl) + sfx
    }
}

fn decode_remaining(cab: &mut CabacReader, rice: u32) -> u32 {
    let mut pfx = 0u32;
    for _ in 0..31 {
        if cab.decode_bypass() == 0 {
            break;
        }
        pfx += 1;
    }
    if pfx < 3 {
        let sfx = cab.decode_bypass_bits(rice);
        (pfx << rice) + sfx
    } else {
        let sl = (pfx - 3 + rice).min(31); // cap to prevent overflow
        let sfx = cab.decode_bypass_bits(sl);
        ((1u32 << sl).wrapping_sub(1u32 << rice.min(31)) + sfx)
            .wrapping_add(3u32.wrapping_shl(rice))
    }
}

/// H.265 Table 8-10: QpC from qPi for 4:2:0 chroma.
fn chroma_qp(qpi: i32) -> i32 {
    const QPC_TABLE: [i32; 58] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 29, 30, 31, 32, 32, 33, 34, 34, 35, 35, 36, 36, 37, 37, 37, 38, 38, 38,
        39, 39, 39, 39, 39, 39, 39, 39, 39, 39,
    ];
    let idx = qpi.clamp(0, 57) as usize;
    QPC_TABLE[idx]
}

fn ceil_log2(x: u32) -> u32 {
    if x <= 1 {
        return 0;
    }
    32 - (x - 1).leading_zeros()
}
