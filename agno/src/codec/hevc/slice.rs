use anyhow::Result;

use super::bitstream::BitReader;
use super::intra::{Component, predict_intra};
use super::params::{Pps, Sps};
use super::picture::{CtuSaoParams, Picture, SaoParams};
use super::transform;

// ---------------------------------------------------------------------------
// CABAC engine (ITU-T H.265 section 9.3)
// ---------------------------------------------------------------------------

const TRANS_IDX_LPS: [u8; 64] = [
    0, 0, 1, 2, 2, 4, 4, 5, 6, 7, 8, 9, 9, 11, 11, 12, 13, 13, 15, 15, 16, 16, 18, 18, 19, 19,
    21, 21, 22, 22, 23, 24, 24, 25, 26, 26, 27, 27, 28, 29, 29, 30, 30, 30, 31, 32, 32, 33, 33,
    33, 34, 34, 35, 35, 35, 36, 36, 36, 37, 37, 37, 38, 38, 63,
];

const TRANS_IDX_MPS: [u8; 64] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48,
    49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 62, 63,
];

const RANGE_TAB_LPS: [[u16; 4]; 64] = [
    [128,176,208,240],[128,167,197,227],[128,158,187,216],[123,150,178,205],
    [116,142,169,195],[111,135,160,185],[105,128,152,175],[100,122,144,166],
    [95,116,137,158],[90,110,130,150],[85,104,123,142],[81,99,117,135],
    [77,94,111,128],[73,89,105,122],[69,85,100,116],[66,80,95,110],
    [62,76,90,104],[59,72,86,99],[56,69,81,94],[53,65,77,89],
    [51,62,73,85],[48,59,69,80],[46,56,66,76],[43,53,63,72],
    [41,50,59,69],[39,48,56,65],[37,45,54,62],[35,43,51,59],
    [33,41,48,56],[32,39,46,53],[30,37,43,50],[29,35,41,48],
    [27,33,39,45],[26,31,37,43],[24,30,35,41],[23,28,33,39],
    [22,27,32,37],[21,26,30,35],[20,24,29,33],[19,23,27,31],
    [18,22,26,30],[17,21,25,28],[16,20,23,27],[15,19,22,25],
    [14,18,21,24],[14,17,20,23],[13,16,19,22],[12,15,18,21],
    [12,14,17,20],[11,14,16,19],[11,13,15,18],[10,12,15,17],
    [10,12,14,16],[9,11,13,15],[9,11,12,14],[8,10,12,14],
    [8,9,11,13],[7,9,11,12],[7,9,10,12],[7,8,10,11],
    [6,8,9,11],[6,7,9,10],[6,7,8,9],[2,2,2,2],
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
        self.ctxs.clear();
        for _ in 0..256 {
            let iv = 154i32;
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

    fn decode_decision(&mut self, ctx_idx: usize) -> u8 {
        let q = ((self.range >> 6) & 3) as usize;
        let lps_range = RANGE_TAB_LPS[self.ctxs[ctx_idx].state as usize][q] as u32;
        self.range -= lps_range;

        let st = self.ctxs[ctx_idx].state;
        let mp = self.ctxs[ctx_idx].mps;

        if self.offset >= self.range {
            self.offset -= self.range;
            self.range = lps_range;
            let val = 1 - mp;
            if st == 0 {
                self.ctxs[ctx_idx].mps = 1 - mp;
            }
            self.ctxs[ctx_idx].state = TRANS_IDX_LPS[st as usize];
            self.renorm();
            val
        } else {
            self.ctxs[ctx_idx].state = TRANS_IDX_MPS[st as usize];
            self.renorm();
            mp
        }
    }

    fn decode_bypass(&mut self) -> u8 {
        self.offset = (self.offset << 1) | self.read_raw(1);
        if self.offset >= self.range {
            self.offset -= self.range;
            1
        } else {
            0
        }
    }

    fn decode_bypass_bits(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.decode_bypass() as u32;
        }
        v
    }

    fn decode_terminate(&mut self) -> bool {
        self.range -= 2;
        if self.offset >= self.range {
            true
        } else {
            self.renorm();
            false
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

// Diagonal 4x4 scan (Table 6-5)
const DIAG4: [[u8; 2]; 16] = [
    [0,0],[1,0],[0,1],[0,2],[1,1],[2,0],[3,0],[2,1],
    [1,2],[0,3],[1,3],[2,2],[3,1],[3,2],[2,3],[3,3],
];

/// Decode a single slice from RBSP data and write reconstructed samples into `pic`.
pub fn decode_slice(rbsp: &[u8], sps: &Sps, pps: &Pps, pic: &mut Picture) -> Result<()> {
    let mut br = BitReader::new(rbsp);

    // -- Slice header (minimal for I-slices) --
    let _first_slice = br.read_flag()?;
    let _slice_type = br.read_ue()?;

    if pps.output_flag_present_flag {
        let _ = br.read_flag()?;
    }
    br.skip_bits(pps.num_extra_slice_header_bits as usize)?;

    let slice_qp_delta = br.read_se()?;
    let slice_qp = 26 + pps.init_qp_minus26 + slice_qp_delta;

    let mut cb_qp_off = pps.pps_cb_qp_offset;
    let mut cr_qp_off = pps.pps_cr_qp_offset;
    if pps.pps_slice_chroma_qp_offsets_present_flag {
        cb_qp_off += br.read_se()?;
        cr_qp_off += br.read_se()?;
    }

    if pps.deblocking_filter_override_enabled_flag {
        if br.read_flag()? {
            let dis = br.read_flag()?;
            if !dis {
                let _ = br.read_se()?;
                let _ = br.read_se()?;
            }
        }
    }

    let sao_luma = if sps.sample_adaptive_offset_enabled_flag { br.read_flag()? } else { false };
    let sao_chroma = if sps.sample_adaptive_offset_enabled_flag { br.read_flag()? } else { false };

    // Byte-align for CABAC
    if br.bits_remaining() > 0 {
        let _ = br.read_bit()?; // alignment bit (1)
        while br.bits_remaining() % 8 != 0 {
            let _ = br.read_bit()?;
        }
    }

    let cabac_start = (rbsp.len() * 8 - br.bits_remaining()) / 8;
    let mut cab = CabacReader::new(rbsp, cabac_start);
    cab.init_contexts(slice_qp);

    let ctb = sps.ctb_size();
    let w_ctbs = sps.pic_width_in_ctbs();
    let h_ctbs = sps.pic_height_in_ctbs();
    let total = (w_ctbs * h_ctbs) as usize;

    pic.sao_params.resize(total, CtuSaoParams::default());

    for addr in 0..total {
        let cx = (addr as u32) % w_ctbs;
        let cy = (addr as u32) / w_ctbs;
        let x0 = cx * ctb;
        let y0 = cy * ctb;

        if sps.sample_adaptive_offset_enabled_flag {
            pic.sao_params[addr] = decode_sao(&mut cab, sao_luma, sao_chroma);
        }

        decode_quadtree(&mut cab, pic, sps, pps, x0, y0, ctb, sps.ctb_log2_size(), 0, slice_qp, cb_qp_off, cr_qp_off);

        if cab.decode_terminate() {
            break;
        }
    }

    Ok(())
}

fn decode_sao(cab: &mut CabacReader, luma: bool, chroma: bool) -> CtuSaoParams {
    let mut p = CtuSaoParams::default();
    if cab.decode_decision(CTX_SAO_MERGE) != 0 { return p; }
    if cab.decode_decision(CTX_SAO_MERGE) != 0 { return p; }
    if luma { p.y = decode_sao_comp(cab); }
    if chroma {
        p.cb = decode_sao_comp(cab);
        p.cr = decode_sao_comp(cab);
    }
    p
}

fn decode_sao_comp(cab: &mut CabacReader) -> SaoParams {
    let mut s = SaoParams::default();
    if cab.decode_decision(CTX_SAO_TYPE) == 0 { return s; }
    let tb = cab.decode_bypass();
    s.sao_type_idx = if tb == 0 { 1 } else { 2 };

    for i in 0..4 {
        let mut m = 0i8;
        for _ in 0..7 { if cab.decode_bypass() == 0 { break; } m += 1; }
        s.sao_offset[i] = m;
    }

    if s.sao_type_idx == 1 {
        for i in 0..4 {
            if s.sao_offset[i] != 0 && cab.decode_bypass() != 0 {
                s.sao_offset[i] = -s.sao_offset[i];
            }
        }
        s.sao_band_position = cab.decode_bypass_bits(5) as u8;
    } else {
        s.sao_offset[2] = -s.sao_offset[2].abs();
        s.sao_offset[3] = -s.sao_offset[3].abs();
        s.sao_eo_class = cab.decode_bypass_bits(2) as u8;
    }
    s
}

// ---------------------------------------------------------------------------
// Quadtree + CU + TU
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn decode_quadtree(
    cab: &mut CabacReader, pic: &mut Picture, sps: &Sps, pps: &Pps,
    x0: u32, y0: u32, size: u32, log2: u32, depth: u32,
    qp: i32, cb_qp: i32, cr_qp: i32,
) {
    if x0 >= sps.pic_width_in_luma_samples || y0 >= sps.pic_height_in_luma_samples { return; }

    let split = if log2 > sps.min_cb_log2_size() {
        let forced = x0 + size > sps.pic_width_in_luma_samples
            || y0 + size > sps.pic_height_in_luma_samples;
        if forced { true } else { cab.decode_decision(CTX_SPLIT_CU + depth.min(2) as usize) != 0 }
    } else { false };

    if split {
        let h = size / 2;
        let l = log2 - 1;
        let d = depth + 1;
        decode_quadtree(cab, pic, sps, pps, x0, y0, h, l, d, qp, cb_qp, cr_qp);
        decode_quadtree(cab, pic, sps, pps, x0+h, y0, h, l, d, qp, cb_qp, cr_qp);
        decode_quadtree(cab, pic, sps, pps, x0, y0+h, h, l, d, qp, cb_qp, cr_qp);
        decode_quadtree(cab, pic, sps, pps, x0+h, y0+h, h, l, d, qp, cb_qp, cr_qp);
    } else {
        decode_cu(cab, pic, sps, pps, x0, y0, size, log2, qp, cb_qp, cr_qp);
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_cu(
    cab: &mut CabacReader, pic: &mut Picture, sps: &Sps, pps: &Pps,
    x0: u32, y0: u32, size: u32, log2: u32,
    qp: i32, cb_qp: i32, cr_qp: i32,
) {
    let part = if size == sps.min_cb_size() && log2 > 3 {
        cab.decode_decision(CTX_PART_MODE)
    } else { 0 };

    if part == 0 {
        let lm = decode_intra_mode(cab);
        let cm = decode_chroma_mode(cab, lm);
        decode_tt(cab, pic, sps, pps, x0, y0, log2, 0, lm, cm, qp, cb_qp, cr_qp);
    } else {
        let h = size / 2;
        let sl = log2 - 1;
        let pos = [(x0,y0),(x0+h,y0),(x0,y0+h),(x0+h,y0+h)];
        let mut lm = [0u8;4];
        let mut cm = [0u8;4];
        for i in 0..4 { lm[i] = decode_intra_mode(cab); }
        for i in 0..4 { cm[i] = decode_chroma_mode(cab, lm[i]); }
        for i in 0..4 {
            let (px,py) = pos[i];
            if px < sps.pic_width_in_luma_samples && py < sps.pic_height_in_luma_samples {
                decode_tt(cab, pic, sps, pps, px, py, sl, 0, lm[i], cm[i], qp, cb_qp, cr_qp);
            }
        }
    }
}

fn decode_intra_mode(cab: &mut CabacReader) -> u8 {
    if cab.decode_decision(CTX_PREV_INTRA_PRED) != 0 {
        let idx = if cab.decode_bypass() == 0 { 0 }
            else if cab.decode_bypass() == 0 { 1 } else { 2 };
        [0u8, 1, 26][idx]
    } else {
        let rem = cab.decode_bypass_bits(5) as u8;
        let mut sorted = [0u8, 1, 26];
        sorted.sort();
        let mut m = rem;
        for &mp in &sorted { if m >= mp { m += 1; } }
        m.min(34)
    }
}

fn decode_chroma_mode(cab: &mut CabacReader, luma: u8) -> u8 {
    if cab.decode_decision(CTX_CHROMA_PRED) == 0 {
        luma
    } else {
        match cab.decode_bypass_bits(2) { 0 => 0, 1 => 26, 2 => 10, _ => 1 }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_tt(
    cab: &mut CabacReader, pic: &mut Picture, sps: &Sps, pps: &Pps,
    x0: u32, y0: u32, log2: u32, depth: u32,
    lm: u8, cm: u8, qp: i32, cb_qp: i32, cr_qp: i32,
) {
    if x0 >= sps.pic_width_in_luma_samples || y0 >= sps.pic_height_in_luma_samples { return; }

    let split = if log2 > sps.max_tb_log2_size() { true }
    else if log2 <= sps.min_tb_log2_size() || depth >= sps.max_transform_hierarchy_depth_intra { false }
    else { cab.decode_decision(CTX_SPLIT_TF + (5u32.saturating_sub(log2)).min(2) as usize) != 0 };

    if split {
        let h = 1u32 << (log2 - 1);
        let d = depth + 1;
        let l = log2 - 1;
        if log2 > 3 {
            let _cbf_cb = cab.decode_decision(CTX_CBF_CHROMA + depth.min(3) as usize);
            let _cbf_cr = cab.decode_decision(CTX_CBF_CHROMA + depth.min(3) as usize);
        }
        decode_tt(cab, pic, sps, pps, x0, y0, l, d, lm, cm, qp, cb_qp, cr_qp);
        decode_tt(cab, pic, sps, pps, x0+h, y0, l, d, lm, cm, qp, cb_qp, cr_qp);
        decode_tt(cab, pic, sps, pps, x0, y0+h, l, d, lm, cm, qp, cb_qp, cr_qp);
        decode_tt(cab, pic, sps, pps, x0+h, y0+h, l, d, lm, cm, qp, cb_qp, cr_qp);
    } else {
        decode_tu(cab, pic, sps, x0, y0, log2, depth, lm, cm, qp, cb_qp, cr_qp);
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_tu(
    cab: &mut CabacReader, pic: &mut Picture, sps: &Sps,
    x0: u32, y0: u32, log2: u32, depth: u32,
    lm: u8, cm: u8, qp: i32, cb_qp: i32, cr_qp: i32,
) {
    let size = 1u32 << log2;
    let bd = pic.bit_depth;
    let max_val = (1i32 << bd) - 1;

    let cbf_y = cab.decode_decision(CTX_CBF_LUMA + (depth > 0) as usize) != 0;
    let has_c = log2 > 2;
    let cbf_cb = if has_c { cab.decode_decision(CTX_CBF_CHROMA + depth.min(3) as usize) != 0 } else { false };
    let cbf_cr = if has_c { cab.decode_decision(CTX_CBF_CHROMA + depth.min(3) as usize) != 0 } else { false };

    // Luma
    let pred = predict_intra(pic, x0, y0, size, lm, Component::Y, sps.strong_intra_smoothing_enabled_flag);
    if cbf_y {
        let mut c = vec![0i32; (size*size) as usize];
        decode_residual(cab, &mut c, log2, 0);
        transform::dequantize(&mut c, qp, bd, log2);
        transform::inverse_transform(&mut c, size, size == 4, bd);
        for py in 0..size { for px in 0..size {
            let sx = x0+px; let sy = y0+py;
            if sx < pic.width && sy < pic.height {
                let i = (py*size+px) as usize;
                pic.set_y(sx, sy, (pred[i] as i32 + c[i]).clamp(0, max_val) as i16);
            }
        }}
    } else {
        for py in 0..size { for px in 0..size {
            let sx = x0+px; let sy = y0+py;
            if sx < pic.width && sy < pic.height {
                pic.set_y(sx, sy, pred[(py*size+px) as usize]);
            }
        }}
    }

    if !has_c { return; }
    let cs = size / 2;
    let cl = log2 - 1;
    let cx = x0 / 2;
    let cy = y0 / 2;
    let cw = (pic.width + 1) / 2;
    let ch = (pic.height + 1) / 2;

    // Cb
    let pred_cb = predict_intra(pic, x0, y0, size, cm, Component::Cb, false);
    if cbf_cb {
        let mut c = vec![0i32; (cs*cs) as usize];
        decode_residual(cab, &mut c, cl, 1);
        transform::dequantize(&mut c, cb_qp, bd, cl);
        transform::inverse_transform(&mut c, cs, false, bd);
        for py in 0..cs { for px in 0..cs {
            let sx = cx+px; let sy = cy+py;
            if sx < cw && sy < ch {
                let i = (py*cs+px) as usize;
                pic.set_cb(sx, sy, (pred_cb[i] as i32 + c[i]).clamp(0, max_val) as i16);
            }
        }}
    } else {
        for py in 0..cs { for px in 0..cs {
            let sx = cx+px; let sy = cy+py;
            if sx < cw && sy < ch {
                pic.set_cb(sx, sy, pred_cb[(py*cs+px) as usize]);
            }
        }}
    }

    // Cr
    let pred_cr = predict_intra(pic, x0, y0, size, cm, Component::Cr, false);
    if cbf_cr {
        let mut c = vec![0i32; (cs*cs) as usize];
        decode_residual(cab, &mut c, cl, 2);
        transform::dequantize(&mut c, cr_qp, bd, cl);
        transform::inverse_transform(&mut c, cs, false, bd);
        for py in 0..cs { for px in 0..cs {
            let sx = cx+px; let sy = cy+py;
            if sx < cw && sy < ch {
                let i = (py*cs+px) as usize;
                pic.set_cr(sx, sy, (pred_cr[i] as i32 + c[i]).clamp(0, max_val) as i16);
            }
        }}
    } else {
        for py in 0..cs { for px in 0..cs {
            let sx = cx+px; let sy = cy+py;
            if sx < cw && sy < ch {
                pic.set_cr(sx, sy, pred_cr[(py*cs+px) as usize]);
            }
        }}
    }
}

// ---------------------------------------------------------------------------
// Residual coding
// ---------------------------------------------------------------------------

fn decode_residual(cab: &mut CabacReader, coeffs: &mut [i32], log2: u32, c_idx: u8) {
    let size = 1u32 << log2;
    let ctx_off = if c_idx == 0 { (3*(log2-2)).min(9) as usize } else { 15 };

    let last_x = decode_last_pos(cab, CTX_LAST_X + ctx_off, log2);
    let last_y = decode_last_pos(cab, CTX_LAST_Y + ctx_off, log2);

    let spr = size / 4;
    let last_sub = (last_y / 4) * spr + (last_x / 4);

    for sub_idx in (0..=last_sub).rev() {
        let sx = (sub_idx % spr) * 4;
        let sy = (sub_idx / spr) * 4;

        let coded = if sub_idx == last_sub || sub_idx == 0 { true }
        else { cab.decode_decision(CTX_CODED_SUB + if c_idx == 0 { 0 } else { 2 }) != 0 };

        if !coded { continue; }

        let mut sig = [false; 16];
        let mut _nsig = 0u32;

        for sp in (0..16).rev() {
            let px = DIAG4[sp][0] as u32 + sx;
            let py = DIAG4[sp][1] as u32 + sy;
            if px >= size || py >= size { continue; }
            if px == last_x && py == last_y && sub_idx == last_sub {
                sig[sp] = true; _nsig += 1; continue;
            }
            if sub_idx == last_sub {
                let lp = (last_y % 4) * 4 + (last_x % 4);
                if (DIAG4[sp][1] as u32) * 4 + DIAG4[sp][0] as u32 > lp { continue; }
            }
            let ctx = sig_ctx(c_idx, sp, sx, sy, log2);
            if cab.decode_decision(CTX_SIG_COEFF + ctx) != 0 {
                sig[sp] = true; _nsig += 1;
            }
        }

        let mut abs = [0i32; 16];
        let mut gt1_cnt = 0u32;
        let mut first_gt2: Option<usize> = None;
        let mut first_gt1 = true;
        let mut gt1_ctx = 1usize;

        for sp in (0..16).rev() {
            if !sig[sp] { continue; }
            abs[sp] = 1;
            if gt1_cnt < 8 {
                let cs = if c_idx == 0 { 0 } else { 4 };
                if cab.decode_decision(CTX_GT1 + cs + gt1_ctx) != 0 {
                    abs[sp] = 2;
                    if first_gt1 { first_gt2 = Some(sp); }
                    gt1_ctx = 0;
                    first_gt1 = false;
                } else if first_gt1 {
                    gt1_ctx = (gt1_ctx + 1).min(3);
                }
                gt1_cnt += 1;
            }
        }

        if let Some(p) = first_gt2 {
            let cs = if c_idx == 0 { 0 } else { 1 };
            if cab.decode_decision(CTX_GT2 + cs) != 0 { abs[p] = 3; }
        }

        let mut signs = [false; 16];
        for sp in (0..16).rev() {
            if sig[sp] { signs[sp] = cab.decode_bypass() != 0; }
        }

        for sp in (0..16).rev() {
            if !sig[sp] { continue; }
            if abs[sp] >= 3 || (abs[sp] == 2 && first_gt2 != Some(sp)) {
                abs[sp] += decode_remaining(cab, 0) as i32;
            }
            let c = if signs[sp] { -abs[sp] } else { abs[sp] };
            let px = DIAG4[sp][0] as u32 + sx;
            let py = DIAG4[sp][1] as u32 + sy;
            if px < size && py < size {
                coeffs[(py * size + px) as usize] = c;
            }
        }
    }
}

fn decode_last_pos(cab: &mut CabacReader, ctx_base: usize, log2: u32) -> u32 {
    let max_p = (log2 << 1).min(9);
    let mut pfx = 0u32;
    for i in 0..max_p {
        if cab.decode_decision(ctx_base + (i >> 1).min(5) as usize) == 0 { break; }
        pfx += 1;
    }
    if pfx < 2 { pfx }
    else {
        let sl = (pfx - 2) >> 1;
        let sfx = cab.decode_bypass_bits(sl + 1);
        ((2 + (pfx & 1)) << sl) - 2 + sfx
    }
}

fn decode_remaining(cab: &mut CabacReader, rice: u32) -> u32 {
    let mut pfx = 0u32;
    for _ in 0..32 { if cab.decode_bypass() == 0 { break; } pfx += 1; }
    if pfx < 3 {
        let sfx = cab.decode_bypass_bits(rice);
        (pfx << rice) + sfx
    } else {
        let sl = pfx - 3 + rice + 1;
        let sfx = cab.decode_bypass_bits(sl);
        ((1u32 << sl) - (1u32 << rice) + sfx).wrapping_add(3 << rice)
    }
}

fn sig_ctx(c_idx: u8, sp: usize, sx: u32, sy: u32, log2: u32) -> usize {
    let p = DIAG4[sp][1] as u32 * 4 + DIAG4[sp][0] as u32;
    if c_idx == 0 {
        if log2 == 2 { (p as usize).min(15) }
        else { (if sx == 0 && sy == 0 { 0 } else { 16 }) + (p as usize).min(15) }
    } else {
        32 + (p as usize).min(11)
    }
}
