# HEVC Decoder (H.265 Still-Image)

## Status: CABAC synced, reference sample availability fixed

The native HEVC decoder decodes HEIC still images (single-tile and grid). The CABAC arithmetic engine is verified to match FFmpeg decision-for-decision for all 2656 context-coded decisions in the first WPP row of sideways2.heic. A critical reconstruction bug was fixed: intra prediction was reading uninitialized pixels from not-yet-decoded CUs as reference samples (H.265 8.4.4.2.2 availability violation). Y PSNR improved from ~4.2 dB to ~10.4 dB; the first WPP row now decodes at >40 dB per CTU. The remaining quality gap is from CABAC context-selection bugs in subsequent WPP rows.

## Current Bit Consumption Analysis

| Version | Ratio | Y PSNR | Notes |
|---------|-------|--------|-------|
| Before cu_qp_delta + WPP | 1.17 | 2.48 dB | Missing cu_qp_delta bytes misinterpreted as residual |
| After cu_qp_delta, no WPP | 0.39 | 2.43 dB | Under-consuming: early terminate at CTU 23/256 |
| After cu_qp_delta + WPP | 6.53 | 2.91 dB | WPP processes all 16 rows but each row over-consumes |

## Key Findings (This Session)

### 1. cu_qp_delta decoding (H.265 7.3.8.11)
**Discovery**: `cu_qp_delta_enabled_flag=true` and `diff_cu_qp_delta_depth=2` in the PPS. This means cu_qp_delta_abs must be decoded in every TU that has non-zero cbf, at 8x8 quantization group granularity. The decoder was NOT decoding this, causing the cu_qp_delta bytes to be misinterpreted as residual data.

**Fix**: Added `QpState` struct to track per-QG state, `decode_cu_qp_delta()` function, and wiring through decode_quadtree/decode_cu/decode_tt/decode_tu/decode_tu_nxn.

**Impact on CTU 0**: Before: 192 decisions + 60 bypass. After: 77 decisions + 43 bypass. FFmpeg reference: 161 decisions + 116 bypass. The fix reduced decisions but the decoder is now UNDER-consuming vs FFmpeg (120 vs 277 total ops). The cu_qp_delta consumes 5 decisions + 1 bypass for abs=4, sign=-1, giving qp_delta=-4 (QP=16-4=12).

### 2. WPP (Wavefront Parallel Processing) support
**Discovery**: `entropy_coding_sync_enabled_flag=true` in the PPS. The slice data is partitioned into 16 independent row segments via 15 entry_point_offsets. Without WPP support, the decoder processed all 256 CTUs sequentially with no CABAC state reset, causing cascading desync from row 0 errors.

**Fix**: Added CABAC context save/restore, engine reinitialization at row boundaries, and per-row terminate handling. Entry point offsets are stored and used to seek to the correct byte position for each row.

**Note**: The entry_point_offsets are in coded-stream bytes. For this image, zero emulation prevention bytes were removed (raw 5640 bytes -> RBSP 5634 bytes after header strip, EP removal: no change), so offsets map directly. For images with EP bytes, a mapping would be needed.

### 3. CABAC engine verification
The CABAC arithmetic engine was manually verified for the first 13 decisions of CTU 0. All match the expected values from the H.265 spec:
- D1-D2: SAO type=0,0 (ctx 137, both MPS)
- D3: split_cu=0 (ctx 0, LPS)
- D4: prev_intra=1 (ctx 11, LPS)
- D5: chroma=luma (ctx 12, LPS val=0)
- D6-D7: cbf_cb=1, cbf_cr=1 (ctx 18, MPS)
- D8: cbf_luma=1 (ctx 17, MPS)
- D9-D13: cu_qp_delta_abs=4 (ctx 138-139)

The CU structure for CTU 0 is: single 32x32 CU, planar mode, all cbf=true, qp_delta=-4.

## File Map

| File | Role |
|------|------|
| `codec/hevc/mod.rs` | Top-level `decode_hevc_still`: parses hvcC, extracts NAL units, dispatches slice decoding |
| `codec/hevc/slice.rs` | CABAC engine + all syntax element decoding (SAO, quadtree, CU, TU, residual), QpState, WPP support |
| `codec/hevc/picture.rs` | Picture buffer with YCbCr 4:2:0 planes, color space conversion, per-CU metadata maps |
| `codec/hevc/params.rs` | VPS/SPS/PPS parsing, SPS helper methods (ctb_size, min_cb, etc.) |
| `codec/hevc/intra.rs` | Intra prediction (modes 0-34: planar, DC, angular) |
| `codec/hevc/transform.rs` | Inverse quantization + inverse DCT/DST (4/8/16/32-point) |
| `codec/hevc/filter.rs` | Deblocking filter and SAO post-processing |
| `codec/hevc/bitstream.rs` | BitReader for slice headers (with `new_rbsp` for pre-cleaned data) |
| `tests/hevc_decode_tests.rs` | Integration tests: green ratio, RGB PSNR, per-plane YCbCr PSNR |

## Remaining Quality Issues

CABAC parsing is verified correct for both WPP row 0 (2656 decisions) and WPP row 1 (2813 decisions) -- all context-coded decisions match FFmpeg exactly. Dequantized coefficients and inverse transform residuals also match FFmpeg for verified blocks. Y PSNR is ~24.87 dB (target >30 dB). First 4 WPP rows decode at >60 dB per CTU row; later rows degrade due to CABAC context-selection bugs causing per-substream desync.

### Fixed reconstruction bugs:
- **MPM third candidate formula**: Was `2 + ((a + 30) % 32)` = `2 + ((a-2) % 32)`, must be `2 + ((a - 2 + 1) & 31)` = `2 + ((a-1) & 31)` per H.265 8.4.2
- **MPM above-neighbor CTU row boundary**: Was reading above neighbor mode across CTU row boundaries; must treat above as unavailable (DC=1) when in different CTU row per H.265 8.4.2
- **Mode 10/26 edge filter applied to 32x32 blocks**: Was applied unconditionally; must only apply for luma blocks with size < 32 per H.265 8.4.4.2.7 / FFmpeg pred_template.c line 477
- **Reference sample filtering (H.265 8.4.4.2.3)**: Was completely missing. Added mode-dependent [1,2,1]/4 low-pass filter for luma blocks >= 8x8 when prediction mode is far from pure horizontal (10) or vertical (26). Uses distance threshold table `[7, 1, 0]` indexed by `log2(nTbS)-3`. Strong intra smoothing (32x32 linear interpolation) now correctly checks both top and left deviation independently (was checking a combined condition). First Y divergence moved from (192, 26) to (288, 63).

### Remaining issues:
- Per-WPP-row CABAC desync causes degradation in rows 4+ (each row decodes its own substream independently). Rows 0-3 are >60 dB; rows 4+ have variable quality (row 13 worst at 16 dB). This is NOT cascading intra prediction error -- WPP resets CABAC per row.
- sig_coeff_flag context (simplified derivation)
- coeff_abs_level_greater1 ctxSet (needs previous sub-block state)
- coded_sub_block_flag context (needs neighbor flags)
- These context bugs affect renormalization timing, causing small per-decision bit differences that accumulate within each WPP substream.

## Key SPS/PPS Parameters for Test Images

**sideways2.heic** (grid: 48 tiles of 512x512, 4032x3024 total):
- MinCbLog2=3 (8x8 min CU), CtbLog2=5 (32x32 CTBs)
- MinTBLog2=2, MaxTBLog2=5
- `max_transform_hierarchy_depth_intra=0` -- TT never splits for non-NxN CUs
- NAL type 20 (IDR_N_LP), slice_qp=16
- `cu_qp_delta_enabled_flag=true`, `diff_cu_qp_delta_depth=2` (QG at 8x8 level)
- `entropy_coding_sync_enabled_flag=true` (WPP: 16 CTU rows, 15 entry points)
- transform_skip=false, sign_data_hiding=false
- sample_adaptive_offset_enabled=true

## Previously Fixed CABAC Bugs

- `decode_last_pos` max_p: `((log2 << 1) - 1)`
- `decode_remaining` escape suffix length: `pfx - 3 + rice`
- Sub-block scan order: diagonal (`DIAG_SUB_2X2/4X4/8X8` tables)
- `part_mode` for I-slices at MinCbSize
- NxN transform tree: IntraSplitFlag forces split; chroma at blkIdx=3
- `coeff_abs_level_remaining` for coefficients beyond first 8
- Rice parameter update: uses final level
- `inferSbDcSigCoeffFlag`: middle sub-blocks only
- `cbf_luma` inference: depth > 0 and both chroma cbfs are 0
- Double emulation prevention: `BitReader::new_rbsp()`
- NxN bin ordering: flags-first, then modes
- NxN chroma: single mode for 4:2:0
- SAO eo_class: Cr copies from Cb
- All 5 context selections ported from FFmpeg
- cu_qp_delta base: was `slice_qp + delta`, must be `current_qp + delta` (H.265 8.6.1)
- Reference sample availability: `read_sample` was only checking picture bounds, must also check CU depth map to exclude not-yet-decoded neighbors (H.265 8.4.4.2.2). Fixed by querying `cu_depth_at()` -- if depth is 0xFF (uninitialized), the sample is unavailable and gets substituted. This was the dominant source of error for the first WPP row.

## Reference: H.265 Section Numbers

| Syntax Element | Spec Section | Notes |
|---------------|-------------|-------|
| slice_segment_header | 7.3.6.1 | IDR vs non-IDR affects which fields are present |
| coding_quadtree | 7.3.8.4 | split_cu_flag, part_mode |
| coding_unit | 7.3.8.5 | intra_mode (flags-first ordering), chroma_mode (1 for 4:2:0) |
| transform_tree | 7.3.8.7 | cbf_cb/cbf_cr, split_transform_flag, IntraSplitFlag |
| transform_unit | 7.3.8.11 | cbf_luma, cu_qp_delta_abs, cu_qp_delta_sign_flag, residual_coding |
| residual_coding | 7.3.8.11 | last_sig_coeff, coded_sub_block_flag, sig_coeff_flag, gt1/gt2, signs, remaining |
| SAO | 7.3.8.3 | eo_class decoded for Y and Cb only; Cr copies from Cb |
| CABAC engine | 9.3.3 | decode_decision, decode_bypass, decode_terminate, renormalization |
| Context init | 9.3.2.2 | I-slice init values, slope/offset -> preCtxState |
| WPP | 9.3.2.3 | Context save after CTU column 1, restore at row start |
