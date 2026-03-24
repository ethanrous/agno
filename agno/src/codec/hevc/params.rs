use anyhow::{Result, ensure};

use super::bitstream::BitReader;

// ---------------------------------------------------------------------------
// Profile / Tier / Level
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ProfileTierLevel {
    pub general_profile_space: u8,
    pub general_tier_flag: bool,
    pub general_profile_idc: u8,
    pub general_profile_compatibility_flags: [bool; 32],
    pub general_progressive_source_flag: bool,
    pub general_interlaced_source_flag: bool,
    pub general_non_packed_constraint_flag: bool,
    pub general_frame_only_constraint_flag: bool,
    pub general_level_idc: u8,
    pub sub_layer_profile_present_flag: Vec<bool>,
    pub sub_layer_level_present_flag: Vec<bool>,
}

pub fn parse_profile_tier_level(
    reader: &mut BitReader,
    profile_present: bool,
    max_sub_layers_minus1: u8,
) -> Result<ProfileTierLevel> {
    let mut ptl = ProfileTierLevel::default();

    if profile_present {
        ptl.general_profile_space = reader.read_bits(2)? as u8;
        ptl.general_tier_flag = reader.read_flag()?;
        ptl.general_profile_idc = reader.read_bits(5)? as u8;

        for i in 0..32 {
            ptl.general_profile_compatibility_flags[i] = reader.read_flag()?;
        }

        ptl.general_progressive_source_flag = reader.read_flag()?;
        ptl.general_interlaced_source_flag = reader.read_flag()?;
        ptl.general_non_packed_constraint_flag = reader.read_flag()?;
        ptl.general_frame_only_constraint_flag = reader.read_flag()?;

        // 44 reserved constraint bits (the remaining constraint flags)
        reader.skip_bits(44)?;
    }

    ptl.general_level_idc = reader.read_bits(8)? as u8;

    for _ in 0..max_sub_layers_minus1 {
        ptl.sub_layer_profile_present_flag.push(reader.read_flag()?);
        ptl.sub_layer_level_present_flag.push(reader.read_flag()?);
    }

    // Byte-alignment padding: if max_sub_layers_minus1 > 0, remaining 2-bit
    // slots up to 8 are reserved zero bits.
    if max_sub_layers_minus1 > 0 {
        let reserved_count = (8 - max_sub_layers_minus1) * 2;
        reader.skip_bits(reserved_count as usize)?;
    }

    // Sub-layer profile/level data
    for i in 0..max_sub_layers_minus1 as usize {
        if ptl.sub_layer_profile_present_flag[i] {
            // sub_layer_profile_space, tier_flag, profile_idc, compatibility[32],
            // progressive, interlaced, non_packed, frame_only, 44 reserved
            reader.skip_bits(2 + 1 + 5 + 32 + 4 + 44)?;
        }
        if ptl.sub_layer_level_present_flag[i] {
            reader.skip_bits(8)?; // sub_layer_level_idc
        }
    }

    Ok(ptl)
}

// ---------------------------------------------------------------------------
// Short-term reference picture set
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ShortTermRefPicSet {
    pub num_negative_pics: u32,
    pub num_positive_pics: u32,
    pub delta_poc_s0: Vec<i32>,
    pub used_by_curr_pic_s0: Vec<bool>,
    pub delta_poc_s1: Vec<i32>,
    pub used_by_curr_pic_s1: Vec<bool>,
}

impl ShortTermRefPicSet {
    pub fn num_delta_pocs(&self) -> u32 {
        self.num_negative_pics + self.num_positive_pics
    }
}

/// Parse a single short-term reference picture set.
///
/// For still images (HEIC) these are typically empty, but the reader must
/// advance past them to reach subsequent SPS fields.
pub fn parse_short_term_ref_pic_set(
    reader: &mut BitReader,
    idx: usize,
    num_short_term_ref_pic_sets: u32,
    ref_pic_sets: &[ShortTermRefPicSet],
) -> Result<ShortTermRefPicSet> {
    let mut rps = ShortTermRefPicSet::default();

    let inter_ref_pic_set_prediction_flag = if idx != 0 { reader.read_flag()? } else { false };

    if inter_ref_pic_set_prediction_flag {
        // Predicted from a previous set
        let delta_idx_minus1 = if idx == num_short_term_ref_pic_sets as usize {
            reader.read_ue()?
        } else {
            0
        };

        let ref_idx = idx as i64 - 1 - delta_idx_minus1 as i64;
        ensure!(
            ref_idx >= 0 && (ref_idx as usize) < ref_pic_sets.len(),
            "short_term_ref_pic_set: invalid reference index {}",
            ref_idx
        );
        let ref_rps = &ref_pic_sets[ref_idx as usize];

        let delta_rps_sign = reader.read_flag()?;
        let abs_delta_rps_minus1 = reader.read_ue()?;
        let delta_rps = (1 + abs_delta_rps_minus1 as i32) * if delta_rps_sign { -1 } else { 1 };

        let num_delta_pocs = ref_rps.num_delta_pocs() as usize;

        let mut used_by_curr_pic_flag = Vec::with_capacity(num_delta_pocs + 1);
        let mut use_delta_flag = Vec::with_capacity(num_delta_pocs + 1);

        for _ in 0..=num_delta_pocs {
            let used = reader.read_flag()?;
            used_by_curr_pic_flag.push(used);
            if !used {
                use_delta_flag.push(reader.read_flag()?);
            } else {
                use_delta_flag.push(true);
            }
        }

        // Derive delta POC values from the reference set.
        // Build negative set (delta_poc < 0).
        let mut d_poc_neg: Vec<(i32, bool)> = Vec::new();

        for j in (0..ref_rps.num_negative_pics as usize).rev() {
            let d_poc_val = ref_rps.delta_poc_s0[j] + delta_rps;
            let flag_idx = ref_rps.num_negative_pics as usize - 1 - j;
            if d_poc_val < 0 && use_delta_flag[flag_idx] {
                d_poc_neg.push((d_poc_val, used_by_curr_pic_flag[flag_idx]));
            }
        }

        if delta_rps < 0 && use_delta_flag[num_delta_pocs] {
            d_poc_neg.push((delta_rps, used_by_curr_pic_flag[num_delta_pocs]));
        }

        for j in 0..ref_rps.num_positive_pics as usize {
            let d_poc_val = ref_rps.delta_poc_s1[j] + delta_rps;
            let flag_idx = ref_rps.num_negative_pics as usize + j;
            if d_poc_val < 0 && use_delta_flag[flag_idx] {
                d_poc_neg.push((d_poc_val, used_by_curr_pic_flag[flag_idx]));
            }
        }

        rps.num_negative_pics = d_poc_neg.len() as u32;
        for &(poc, used) in &d_poc_neg {
            rps.delta_poc_s0.push(poc);
            rps.used_by_curr_pic_s0.push(used);
        }

        // Build positive set (delta_poc > 0).
        let mut d_poc_pos: Vec<(i32, bool)> = Vec::new();

        for j in (0..ref_rps.num_negative_pics as usize).rev() {
            let d_poc_val = ref_rps.delta_poc_s0[j] + delta_rps;
            let flag_idx = ref_rps.num_negative_pics as usize - 1 - j;
            if d_poc_val > 0 && use_delta_flag[flag_idx] {
                d_poc_pos.push((d_poc_val, used_by_curr_pic_flag[flag_idx]));
            }
        }

        if delta_rps > 0 && use_delta_flag[num_delta_pocs] {
            d_poc_pos.push((delta_rps, used_by_curr_pic_flag[num_delta_pocs]));
        }

        for j in 0..ref_rps.num_positive_pics as usize {
            let d_poc_val = ref_rps.delta_poc_s1[j] + delta_rps;
            let flag_idx = ref_rps.num_negative_pics as usize + j;
            if d_poc_val > 0 && use_delta_flag[flag_idx] {
                d_poc_pos.push((d_poc_val, used_by_curr_pic_flag[flag_idx]));
            }
        }

        rps.num_positive_pics = d_poc_pos.len() as u32;
        for &(poc, used) in &d_poc_pos {
            rps.delta_poc_s1.push(poc);
            rps.used_by_curr_pic_s1.push(used);
        }
    } else {
        rps.num_negative_pics = reader.read_ue()?;
        rps.num_positive_pics = reader.read_ue()?;

        ensure!(
            rps.num_negative_pics <= 16,
            "num_negative_pics {} exceeds limit",
            rps.num_negative_pics
        );
        ensure!(
            rps.num_positive_pics <= 16,
            "num_positive_pics {} exceeds limit",
            rps.num_positive_pics
        );

        let mut prev = 0i32;
        for _ in 0..rps.num_negative_pics {
            let delta_poc_s0_minus1 = reader.read_ue()?;
            let used = reader.read_flag()?;
            prev -= (delta_poc_s0_minus1 as i32) + 1;
            rps.delta_poc_s0.push(prev);
            rps.used_by_curr_pic_s0.push(used);
        }

        prev = 0;
        for _ in 0..rps.num_positive_pics {
            let delta_poc_s1_minus1 = reader.read_ue()?;
            let used = reader.read_flag()?;
            prev += (delta_poc_s1_minus1 as i32) + 1;
            rps.delta_poc_s1.push(prev);
            rps.used_by_curr_pic_s1.push(used);
        }
    }

    Ok(rps)
}

// ---------------------------------------------------------------------------
// Scaling list data (parse/skip)
// ---------------------------------------------------------------------------

fn parse_scaling_list_data(reader: &mut BitReader) -> Result<()> {
    for size_id in 0..4u8 {
        let count = if size_id == 3 { 2 } else { 6 };
        for matrix_id in 0..count {
            let scaling_list_pred_mode_flag = reader.read_flag()?;
            if !scaling_list_pred_mode_flag {
                let _pred_matrix_id_delta = reader.read_ue()?;
            } else {
                let coef_num = std::cmp::min(64, 1u32 << (4 + (size_id << 1)));
                if size_id > 1 {
                    let _scaling_list_dc_coef = reader.read_se()?;
                }
                for _ in 0..coef_num {
                    let _scaling_list_delta_coef = reader.read_se()?;
                }
            }
            let _ = matrix_id;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// VPS (Video Parameter Set)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Vps {
    pub _vps_video_parameter_set_id: u8,
    pub _vps_base_layer_internal_flag: bool,
    pub _vps_base_layer_available_flag: bool,
    pub _max_layers_minus1: u8,
    pub max_sub_layers_minus1: u8,
    pub _temporal_id_nesting_flag: bool,
    pub _profile_tier_level: ProfileTierLevel,
}

impl Vps {
    /// Parse a VPS NAL unit from RBSP bits via the given BitReader.
    pub fn parse(reader: &mut BitReader) -> Result<Self> {
        let vps_video_parameter_set_id = reader.read_bits(4)? as u8;
        let vps_base_layer_internal_flag = reader.read_flag()?;
        let vps_base_layer_available_flag = reader.read_flag()?;
        let max_layers_minus1 = reader.read_bits(6)? as u8;
        let max_sub_layers_minus1 = reader.read_bits(3)? as u8;
        let temporal_id_nesting_flag = reader.read_flag()?;

        // 16 reserved 0xFFFF bits
        reader.skip_bits(16)?;

        let profile_tier_level = parse_profile_tier_level(reader, true, max_sub_layers_minus1)?;

        let vps_sub_layer_ordering_info_present_flag = reader.read_flag()?;
        let start = if vps_sub_layer_ordering_info_present_flag {
            0
        } else {
            max_sub_layers_minus1
        };
        for _ in start..=max_sub_layers_minus1 {
            let _max_dec_pic_buffering = reader.read_ue()?;
            let _max_num_reorder_pics = reader.read_ue()?;
            let _max_latency_increase = reader.read_ue()?;
        }

        // Remaining VPS fields (max_layer_id, num_layer_sets, timing info,
        // extensions) are not needed for decoding. Stop here.

        Ok(Self {
            _vps_video_parameter_set_id: vps_video_parameter_set_id,
            _vps_base_layer_internal_flag: vps_base_layer_internal_flag,
            _vps_base_layer_available_flag: vps_base_layer_available_flag,
            _max_layers_minus1: max_layers_minus1,
            max_sub_layers_minus1,
            _temporal_id_nesting_flag: temporal_id_nesting_flag,
            _profile_tier_level: profile_tier_level,
        })
    }
}

// ---------------------------------------------------------------------------
// SPS (Sequence Parameter Set)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Sps {
    pub _sps_video_parameter_set_id: u8,
    pub _sps_max_sub_layers_minus1: u8,
    pub _sps_temporal_id_nesting_flag: bool,
    pub _profile_tier_level: ProfileTierLevel,
    pub _sps_seq_parameter_set_id: u32,
    pub _chroma_format_idc: u32,
    pub _separate_colour_plane_flag: bool,
    pub pic_width_in_luma_samples: u32,
    pub pic_height_in_luma_samples: u32,
    pub _conformance_window_flag: bool,
    pub _conf_win_left_offset: u32,
    pub _conf_win_right_offset: u32,
    pub _conf_win_top_offset: u32,
    pub _conf_win_bottom_offset: u32,
    pub bit_depth_luma_minus8: u32,
    pub _bit_depth_chroma_minus8: u32,
    /// Computed: bit_depth_luma_minus8 + 8.
    pub bit_depth_luma: u8,
    /// Computed: bit_depth_chroma_minus8 + 8.
    pub bit_depth_chroma: u8,
    pub _log2_max_pic_order_cnt_lsb_minus4: u32,
    pub _sps_sub_layer_ordering_info_present_flag: bool,
    pub log2_min_luma_coding_block_size_minus3: u32,
    pub log2_diff_max_min_luma_coding_block_size: u32,
    pub log2_min_luma_transform_block_size_minus2: u32,
    pub log2_diff_max_min_luma_transform_block_size: u32,
    pub _max_transform_hierarchy_depth_inter: u32,
    pub max_transform_hierarchy_depth_intra: u32,
    pub scaling_list_enabled_flag: bool,
    pub _amp_enabled_flag: bool,
    pub sample_adaptive_offset_enabled_flag: bool,
    pub _pcm_enabled_flag: bool,
    pub _pcm_sample_bit_depth_luma_minus1: u8,
    pub _pcm_sample_bit_depth_chroma_minus1: u8,
    pub _log2_min_pcm_luma_coding_block_size_minus3: u32,
    pub _log2_diff_max_min_pcm_luma_coding_block_size: u32,
    pub _pcm_loop_filter_disabled_flag: bool,
    pub _num_short_term_ref_pic_sets: u32,
    pub _short_term_ref_pic_sets: Vec<ShortTermRefPicSet>,
    pub _long_term_ref_pics_present_flag: bool,
    pub _num_long_term_ref_pics_sps: u32,
    pub _sps_temporal_mvp_enabled_flag: bool,
    pub strong_intra_smoothing_enabled_flag: bool,
    pub _vui_parameters_present_flag: bool,
}

impl Sps {
    /// Log2 of the CTB (coding tree block) size.
    pub fn ctb_log2_size(&self) -> u32 {
        self.log2_min_luma_coding_block_size_minus3
            + 3
            + self.log2_diff_max_min_luma_coding_block_size
    }

    /// CTB size in luma samples.
    pub fn ctb_size(&self) -> u32 {
        1 << self.ctb_log2_size()
    }

    /// Picture width in CTB units (rounded up).
    pub fn pic_width_in_ctbs(&self) -> u32 {
        let ctb = self.ctb_size();
        (self.pic_width_in_luma_samples + ctb - 1) / ctb
    }

    /// Picture height in CTB units (rounded up).
    pub fn pic_height_in_ctbs(&self) -> u32 {
        let ctb = self.ctb_size();
        (self.pic_height_in_luma_samples + ctb - 1) / ctb
    }

    /// Log2 of the minimum coding block size.
    pub fn min_cb_log2_size(&self) -> u32 {
        self.log2_min_luma_coding_block_size_minus3 + 3
    }

    /// Minimum coding block size in luma samples.
    pub fn min_cb_size(&self) -> u32 {
        1 << self.min_cb_log2_size()
    }

    /// Log2 of the minimum transform block size.
    pub fn min_tb_log2_size(&self) -> u32 {
        self.log2_min_luma_transform_block_size_minus2 + 2
    }

    /// Log2 of the maximum transform block size.
    pub fn max_tb_log2_size(&self) -> u32 {
        self.min_tb_log2_size() + self.log2_diff_max_min_luma_transform_block_size
    }

    /// Parse an SPS NAL unit from RBSP bits via the given BitReader.
    ///
    /// `max_sub_layers_minus1` is provided by the VPS (or can be read from the
    /// SPS header itself; the spec allows both). It controls how much
    /// profile_tier_level and sub-layer ordering data to parse.
    pub fn parse(reader: &mut BitReader, _max_sub_layers_minus1: u8) -> Result<Self> {
        let sps_video_parameter_set_id = reader.read_bits(4)? as u8;
        let sps_max_sub_layers_minus1 = reader.read_bits(3)? as u8;
        let sps_temporal_id_nesting_flag = reader.read_flag()?;

        let profile_tier_level = parse_profile_tier_level(reader, true, sps_max_sub_layers_minus1)?;

        let sps_seq_parameter_set_id = reader.read_ue()?;
        let chroma_format_idc = reader.read_ue()?;

        let separate_colour_plane_flag = if chroma_format_idc == 3 {
            reader.read_flag()?
        } else {
            false
        };

        let pic_width_in_luma_samples = reader.read_ue()?;
        let pic_height_in_luma_samples = reader.read_ue()?;

        let conformance_window_flag = reader.read_flag()?;
        let (
            conf_win_left_offset,
            conf_win_right_offset,
            conf_win_top_offset,
            conf_win_bottom_offset,
        ) = if conformance_window_flag {
            (
                reader.read_ue()?,
                reader.read_ue()?,
                reader.read_ue()?,
                reader.read_ue()?,
            )
        } else {
            (0, 0, 0, 0)
        };

        let bit_depth_luma_minus8 = reader.read_ue()?;
        let bit_depth_chroma_minus8 = reader.read_ue()?;
        let log2_max_pic_order_cnt_lsb_minus4 = reader.read_ue()?;

        let sps_sub_layer_ordering_info_present_flag = reader.read_flag()?;
        let start = if sps_sub_layer_ordering_info_present_flag {
            0
        } else {
            sps_max_sub_layers_minus1
        };
        for _ in start..=sps_max_sub_layers_minus1 {
            let _max_dec_pic_buffering = reader.read_ue()?;
            let _max_num_reorder_pics = reader.read_ue()?;
            let _max_latency_increase = reader.read_ue()?;
        }

        let log2_min_luma_coding_block_size_minus3 = reader.read_ue()?;
        let log2_diff_max_min_luma_coding_block_size = reader.read_ue()?;
        let log2_min_luma_transform_block_size_minus2 = reader.read_ue()?;
        let log2_diff_max_min_luma_transform_block_size = reader.read_ue()?;
        let max_transform_hierarchy_depth_inter = reader.read_ue()?;
        let max_transform_hierarchy_depth_intra = reader.read_ue()?;

        let scaling_list_enabled_flag = reader.read_flag()?;
        if scaling_list_enabled_flag {
            let sps_scaling_list_data_present_flag = reader.read_flag()?;
            if sps_scaling_list_data_present_flag {
                tracing::warn!("SPS has custom scaling list data — currently discarded");
                parse_scaling_list_data(reader)?;
            }
        }

        let amp_enabled_flag = reader.read_flag()?;
        let sample_adaptive_offset_enabled_flag = reader.read_flag()?;

        let pcm_enabled_flag = reader.read_flag()?;
        let mut pcm_sample_bit_depth_luma_minus1 = 0u8;
        let mut pcm_sample_bit_depth_chroma_minus1 = 0u8;
        let mut log2_min_pcm_luma_coding_block_size_minus3 = 0u32;
        let mut log2_diff_max_min_pcm_luma_coding_block_size = 0u32;
        let mut pcm_loop_filter_disabled_flag = false;

        if pcm_enabled_flag {
            pcm_sample_bit_depth_luma_minus1 = reader.read_bits(4)? as u8;
            pcm_sample_bit_depth_chroma_minus1 = reader.read_bits(4)? as u8;
            log2_min_pcm_luma_coding_block_size_minus3 = reader.read_ue()?;
            log2_diff_max_min_pcm_luma_coding_block_size = reader.read_ue()?;
            pcm_loop_filter_disabled_flag = reader.read_flag()?;
        }

        let num_short_term_ref_pic_sets = reader.read_ue()?;
        ensure!(
            num_short_term_ref_pic_sets <= 64,
            "num_short_term_ref_pic_sets {} exceeds maximum of 64",
            num_short_term_ref_pic_sets
        );

        let mut short_term_ref_pic_sets = Vec::with_capacity(num_short_term_ref_pic_sets as usize);
        for i in 0..num_short_term_ref_pic_sets as usize {
            let rps = parse_short_term_ref_pic_set(
                reader,
                i,
                num_short_term_ref_pic_sets,
                &short_term_ref_pic_sets,
            )?;
            short_term_ref_pic_sets.push(rps);
        }

        let long_term_ref_pics_present_flag = reader.read_flag()?;
        let mut num_long_term_ref_pics_sps = 0u32;
        if long_term_ref_pics_present_flag {
            num_long_term_ref_pics_sps = reader.read_ue()?;
            let lt_bits = log2_max_pic_order_cnt_lsb_minus4 + 4;
            for _ in 0..num_long_term_ref_pics_sps {
                reader.skip_bits(lt_bits as usize)?; // lt_ref_pic_poc_lsb_sps
                reader.skip_bits(1)?; // used_by_curr_pic_lt_sps_flag
            }
        }

        let sps_temporal_mvp_enabled_flag = reader.read_flag()?;
        let strong_intra_smoothing_enabled_flag = reader.read_flag()?;

        let vui_parameters_present_flag = reader.read_flag()?;
        // VUI parsing is not required for decoding still images.
        // The flag is stored so callers know it was present.

        Ok(Self {
            _sps_video_parameter_set_id: sps_video_parameter_set_id,
            _sps_max_sub_layers_minus1: sps_max_sub_layers_minus1,
            _sps_temporal_id_nesting_flag: sps_temporal_id_nesting_flag,
            _profile_tier_level: profile_tier_level,
            _sps_seq_parameter_set_id: sps_seq_parameter_set_id,
            _chroma_format_idc: chroma_format_idc,
            _separate_colour_plane_flag: separate_colour_plane_flag,
            pic_width_in_luma_samples,
            pic_height_in_luma_samples,
            _conformance_window_flag: conformance_window_flag,
            _conf_win_left_offset: conf_win_left_offset,
            _conf_win_right_offset: conf_win_right_offset,
            _conf_win_top_offset: conf_win_top_offset,
            _conf_win_bottom_offset: conf_win_bottom_offset,
            bit_depth_luma_minus8,
            _bit_depth_chroma_minus8: bit_depth_chroma_minus8,
            bit_depth_luma: (bit_depth_luma_minus8 + 8) as u8,
            bit_depth_chroma: (bit_depth_chroma_minus8 + 8) as u8,
            _log2_max_pic_order_cnt_lsb_minus4: log2_max_pic_order_cnt_lsb_minus4,
            _sps_sub_layer_ordering_info_present_flag: sps_sub_layer_ordering_info_present_flag,
            log2_min_luma_coding_block_size_minus3,
            log2_diff_max_min_luma_coding_block_size,
            log2_min_luma_transform_block_size_minus2,
            log2_diff_max_min_luma_transform_block_size,
            _max_transform_hierarchy_depth_inter: max_transform_hierarchy_depth_inter,
            max_transform_hierarchy_depth_intra,
            scaling_list_enabled_flag: scaling_list_enabled_flag,
            _amp_enabled_flag: amp_enabled_flag,
            sample_adaptive_offset_enabled_flag,
            _pcm_enabled_flag: pcm_enabled_flag,
            _pcm_sample_bit_depth_luma_minus1: pcm_sample_bit_depth_luma_minus1,
            _pcm_sample_bit_depth_chroma_minus1: pcm_sample_bit_depth_chroma_minus1,
            _log2_min_pcm_luma_coding_block_size_minus3: log2_min_pcm_luma_coding_block_size_minus3,
            _log2_diff_max_min_pcm_luma_coding_block_size:
                log2_diff_max_min_pcm_luma_coding_block_size,
            _pcm_loop_filter_disabled_flag: pcm_loop_filter_disabled_flag,
            _num_short_term_ref_pic_sets: num_short_term_ref_pic_sets,
            _short_term_ref_pic_sets: short_term_ref_pic_sets,
            _long_term_ref_pics_present_flag: long_term_ref_pics_present_flag,
            _num_long_term_ref_pics_sps: num_long_term_ref_pics_sps,
            _sps_temporal_mvp_enabled_flag: sps_temporal_mvp_enabled_flag,
            strong_intra_smoothing_enabled_flag,
            _vui_parameters_present_flag: vui_parameters_present_flag,
        })
    }
}

// ---------------------------------------------------------------------------
// PPS (Picture Parameter Set)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Pps {
    pub _pps_pic_parameter_set_id: u32,
    pub _pps_seq_parameter_set_id: u32,
    pub _dependent_slice_segments_enabled_flag: bool,
    pub output_flag_present_flag: bool,
    pub num_extra_slice_header_bits: u8,
    pub sign_data_hiding_enabled_flag: bool,
    pub _cabac_init_present_flag: bool,
    pub _num_ref_idx_l0_default_active_minus1: u32,
    pub _num_ref_idx_l1_default_active_minus1: u32,
    pub init_qp_minus26: i32,
    pub _constrained_intra_pred_flag: bool,
    pub transform_skip_enabled_flag: bool,
    pub _cu_qp_delta_enabled_flag: bool,
    pub _diff_cu_qp_delta_depth: u32,
    pub pps_cb_qp_offset: i32,
    pub pps_cr_qp_offset: i32,
    pub pps_slice_chroma_qp_offsets_present_flag: bool,
    pub _weighted_pred_flag: bool,
    pub _weighted_bipred_flag: bool,
    pub _transquant_bypass_enabled_flag: bool,
    pub _tiles_enabled_flag: bool,
    pub _entropy_coding_sync_enabled_flag: bool,
    pub _num_tile_columns_minus1: u32,
    pub _num_tile_rows_minus1: u32,
    pub _uniform_spacing_flag: bool,
    pub _loop_filter_across_tiles_enabled_flag: bool,
    pub _loop_filter_across_slices_enabled_flag: bool,
    pub _deblocking_filter_control_present_flag: bool,
    pub deblocking_filter_override_enabled_flag: bool,
    pub pps_deblocking_filter_disabled_flag: bool,
    pub pps_beta_offset_div2: i32,
    pub pps_tc_offset_div2: i32,
    pub _pps_scaling_list_data_present_flag: bool,
    pub _lists_modification_present_flag: bool,
    pub _log2_parallel_merge_level_minus2: u32,
    pub _slice_segment_header_extension_present_flag: bool,
}

impl Pps {
    /// Parse a PPS NAL unit from RBSP bits via the given BitReader.
    pub fn parse(reader: &mut BitReader) -> Result<Self> {
        let pps_pic_parameter_set_id = reader.read_ue()?;
        let pps_seq_parameter_set_id = reader.read_ue()?;
        let dependent_slice_segments_enabled_flag = reader.read_flag()?;
        let output_flag_present_flag = reader.read_flag()?;
        let num_extra_slice_header_bits = reader.read_bits(3)? as u8;
        let sign_data_hiding_enabled_flag = reader.read_flag()?;
        let cabac_init_present_flag = reader.read_flag()?;
        let num_ref_idx_l0_default_active_minus1 = reader.read_ue()?;
        let num_ref_idx_l1_default_active_minus1 = reader.read_ue()?;
        let init_qp_minus26 = reader.read_se()?;
        let constrained_intra_pred_flag = reader.read_flag()?;
        let transform_skip_enabled_flag = reader.read_flag()?;

        let cu_qp_delta_enabled_flag = reader.read_flag()?;
        let diff_cu_qp_delta_depth = if cu_qp_delta_enabled_flag {
            reader.read_ue()?
        } else {
            0
        };

        let pps_cb_qp_offset = reader.read_se()?;
        let pps_cr_qp_offset = reader.read_se()?;
        let pps_slice_chroma_qp_offsets_present_flag = reader.read_flag()?;
        let weighted_pred_flag = reader.read_flag()?;
        let weighted_bipred_flag = reader.read_flag()?;
        let transquant_bypass_enabled_flag = reader.read_flag()?;
        let tiles_enabled_flag = reader.read_flag()?;
        let entropy_coding_sync_enabled_flag = reader.read_flag()?;

        let mut num_tile_columns_minus1 = 0u32;
        let mut num_tile_rows_minus1 = 0u32;
        let mut uniform_spacing_flag = true;
        let mut loop_filter_across_tiles_enabled_flag = true;

        if tiles_enabled_flag {
            num_tile_columns_minus1 = reader.read_ue()?;
            num_tile_rows_minus1 = reader.read_ue()?;
            uniform_spacing_flag = reader.read_flag()?;

            if !uniform_spacing_flag {
                for _ in 0..num_tile_columns_minus1 {
                    let _column_width_minus1 = reader.read_ue()?;
                }
                for _ in 0..num_tile_rows_minus1 {
                    let _row_height_minus1 = reader.read_ue()?;
                }
            }

            loop_filter_across_tiles_enabled_flag = reader.read_flag()?;
        }

        let loop_filter_across_slices_enabled_flag = reader.read_flag()?;

        let deblocking_filter_control_present_flag = reader.read_flag()?;
        let mut deblocking_filter_override_enabled_flag = false;
        let mut pps_deblocking_filter_disabled_flag = false;
        let mut pps_beta_offset_div2 = 0i32;
        let mut pps_tc_offset_div2 = 0i32;

        if deblocking_filter_control_present_flag {
            deblocking_filter_override_enabled_flag = reader.read_flag()?;
            pps_deblocking_filter_disabled_flag = reader.read_flag()?;

            if !pps_deblocking_filter_disabled_flag {
                pps_beta_offset_div2 = reader.read_se()?;
                pps_tc_offset_div2 = reader.read_se()?;
            }
        }

        let pps_scaling_list_data_present_flag = reader.read_flag()?;
        if pps_scaling_list_data_present_flag {
            parse_scaling_list_data(reader)?;
        }

        let lists_modification_present_flag = reader.read_flag()?;
        let log2_parallel_merge_level_minus2 = reader.read_ue()?;
        let slice_segment_header_extension_present_flag = reader.read_flag()?;

        Ok(Self {
            _pps_pic_parameter_set_id: pps_pic_parameter_set_id,
            _pps_seq_parameter_set_id: pps_seq_parameter_set_id,
            _dependent_slice_segments_enabled_flag: dependent_slice_segments_enabled_flag,
            output_flag_present_flag,
            num_extra_slice_header_bits,
            sign_data_hiding_enabled_flag,
            _cabac_init_present_flag: cabac_init_present_flag,
            _num_ref_idx_l0_default_active_minus1: num_ref_idx_l0_default_active_minus1,
            _num_ref_idx_l1_default_active_minus1: num_ref_idx_l1_default_active_minus1,
            init_qp_minus26,
            _constrained_intra_pred_flag: constrained_intra_pred_flag,
            transform_skip_enabled_flag,
            _cu_qp_delta_enabled_flag: cu_qp_delta_enabled_flag,
            _diff_cu_qp_delta_depth: diff_cu_qp_delta_depth,
            pps_cb_qp_offset,
            pps_cr_qp_offset,
            pps_slice_chroma_qp_offsets_present_flag,
            _weighted_pred_flag: weighted_pred_flag,
            _weighted_bipred_flag: weighted_bipred_flag,
            _transquant_bypass_enabled_flag: transquant_bypass_enabled_flag,
            _tiles_enabled_flag: tiles_enabled_flag,
            _entropy_coding_sync_enabled_flag: entropy_coding_sync_enabled_flag,
            _num_tile_columns_minus1: num_tile_columns_minus1,
            _num_tile_rows_minus1: num_tile_rows_minus1,
            _uniform_spacing_flag: uniform_spacing_flag,
            _loop_filter_across_tiles_enabled_flag: loop_filter_across_tiles_enabled_flag,
            _loop_filter_across_slices_enabled_flag: loop_filter_across_slices_enabled_flag,
            _deblocking_filter_control_present_flag: deblocking_filter_control_present_flag,
            deblocking_filter_override_enabled_flag,
            pps_deblocking_filter_disabled_flag,
            pps_beta_offset_div2,
            pps_tc_offset_div2,
            _pps_scaling_list_data_present_flag: pps_scaling_list_data_present_flag,
            _lists_modification_present_flag: lists_modification_present_flag,
            _log2_parallel_merge_level_minus2: log2_parallel_merge_level_minus2,
            _slice_segment_header_extension_present_flag:
                slice_segment_header_extension_present_flag,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: accumulate bits MSB-first into a byte vector.
    struct BitWriter {
        bytes: Vec<u8>,
        buf: u64,
        buf_bits: u8,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                buf: 0,
                buf_bits: 0,
            }
        }

        fn write(&mut self, val: u64, n: u8) {
            for i in (0..n).rev() {
                self.buf = (self.buf << 1) | ((val >> i) & 1);
                self.buf_bits += 1;
                if self.buf_bits == 8 {
                    self.bytes.push(self.buf as u8);
                    self.buf = 0;
                    self.buf_bits = 0;
                }
            }
        }

        fn write_ue(&mut self, val: u32) {
            let code = val + 1;
            let n = 32 - code.leading_zeros();
            let leading_zeros = n - 1;
            for _ in 0..leading_zeros {
                self.write(0, 1);
            }
            self.write(code as u64, n as u8);
        }

        fn write_se(&mut self, val: i32) {
            let ue_val = if val <= 0 {
                (-val * 2) as u32
            } else {
                (val * 2 - 1) as u32
            };
            self.write_ue(ue_val);
        }

        fn finish(mut self) -> Vec<u8> {
            if self.buf_bits > 0 {
                self.bytes.push((self.buf << (8 - self.buf_bits)) as u8);
            }
            self.bytes
        }
    }

    #[test]
    fn sps_computed_helpers() {
        let sps = Sps {
            _sps_video_parameter_set_id: 0,
            _sps_max_sub_layers_minus1: 0,
            _sps_temporal_id_nesting_flag: true,
            _profile_tier_level: ProfileTierLevel::default(),
            _sps_seq_parameter_set_id: 0,
            _chroma_format_idc: 1,
            _separate_colour_plane_flag: false,
            pic_width_in_luma_samples: 4032,
            pic_height_in_luma_samples: 3024,
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
            log2_diff_max_min_luma_coding_block_size: 2,
            log2_min_luma_transform_block_size_minus2: 0,
            log2_diff_max_min_luma_transform_block_size: 3,
            _max_transform_hierarchy_depth_inter: 1,
            max_transform_hierarchy_depth_intra: 1,
            scaling_list_enabled_flag: false,
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
        };

        assert_eq!(sps.ctb_log2_size(), 5);
        assert_eq!(sps.ctb_size(), 32);
        assert_eq!(sps.pic_width_in_ctbs(), 126);
        assert_eq!(sps.pic_height_in_ctbs(), 95);
        assert_eq!(sps.min_cb_log2_size(), 3);
        assert_eq!(sps.min_cb_size(), 8);
        assert_eq!(sps.min_tb_log2_size(), 2);
        assert_eq!(sps.max_tb_log2_size(), 5);
    }

    /// Build a minimal SPS bitstream for a 1920x1080 still image and verify
    /// that parsing recovers the dimensions and coding parameters.
    #[test]
    fn parse_synthetic_sps() {
        let mut w = BitWriter::new();

        // sps_video_parameter_set_id = 0 (4 bits)
        w.write(0, 4);
        // sps_max_sub_layers_minus1 = 0 (3 bits)
        w.write(0, 3);
        // sps_temporal_id_nesting_flag = 1
        w.write(1, 1);

        // profile_tier_level (profile_present=true, max_sub_layers_minus1=0)
        w.write(0, 2); // general_profile_space
        w.write(0, 1); // general_tier_flag
        w.write(1, 5); // general_profile_idc = Main
        // general_profile_compatibility_flags[32]
        w.write(0b01000000_00000000_00000000_00000000u64, 32);
        w.write(0b1011, 4); // progressive, !interlaced, non_packed, frame_only
        w.write(0, 44); // reserved constraint bits
        w.write(120, 8); // general_level_idc = Level 4.0

        // SPS fields
        w.write_ue(0); // sps_seq_parameter_set_id
        w.write_ue(1); // chroma_format_idc = 4:2:0
        w.write_ue(1920); // pic_width_in_luma_samples
        w.write_ue(1080); // pic_height_in_luma_samples
        w.write(0, 1); // conformance_window_flag = 0
        w.write_ue(0); // bit_depth_luma_minus8
        w.write_ue(0); // bit_depth_chroma_minus8
        w.write_ue(4); // log2_max_pic_order_cnt_lsb_minus4
        w.write(0, 1); // sps_sub_layer_ordering_info_present_flag
        w.write_ue(1); // max_dec_pic_buffering
        w.write_ue(0); // max_num_reorder_pics
        w.write_ue(0); // max_latency_increase
        w.write_ue(0); // log2_min_luma_coding_block_size_minus3
        w.write_ue(3); // log2_diff_max_min => CTB=64
        w.write_ue(0); // log2_min_luma_transform_block_size_minus2
        w.write_ue(3); // log2_diff_max_min_luma_transform_block_size
        w.write_ue(1); // max_transform_hierarchy_depth_inter
        w.write_ue(1); // max_transform_hierarchy_depth_intra
        w.write(0, 1); // scaling_list_enabled_flag
        w.write(1, 1); // amp_enabled_flag
        w.write(1, 1); // sample_adaptive_offset_enabled_flag
        w.write(0, 1); // pcm_enabled_flag
        w.write_ue(0); // num_short_term_ref_pic_sets
        w.write(0, 1); // long_term_ref_pics_present_flag
        w.write(1, 1); // sps_temporal_mvp_enabled_flag
        w.write(1, 1); // strong_intra_smoothing_enabled_flag
        w.write(0, 1); // vui_parameters_present_flag

        let data = w.finish();
        let mut reader = BitReader::new(&data);
        let sps = Sps::parse(&mut reader, 0).expect("SPS parse should succeed");

        assert_eq!(sps.pic_width_in_luma_samples, 1920);
        assert_eq!(sps.pic_height_in_luma_samples, 1080);
        assert_eq!(sps._chroma_format_idc, 1);
        assert_eq!(sps.bit_depth_luma, 8);
        assert_eq!(sps.bit_depth_chroma, 8);
        assert_eq!(sps.ctb_log2_size(), 6);
        assert_eq!(sps.ctb_size(), 64);
        assert_eq!(sps.pic_width_in_ctbs(), 30);
        assert_eq!(sps.pic_height_in_ctbs(), 17);
        assert_eq!(sps.min_cb_log2_size(), 3);
        assert_eq!(sps.min_cb_size(), 8);
        assert!(!sps._conformance_window_flag);
        assert!(sps._amp_enabled_flag);
        assert!(sps._sps_temporal_mvp_enabled_flag);
    }

    #[test]
    fn parse_synthetic_vps() {
        let mut w = BitWriter::new();

        w.write(0, 4); // vps_video_parameter_set_id
        w.write(1, 1); // vps_base_layer_internal_flag
        w.write(1, 1); // vps_base_layer_available_flag
        w.write(0, 6); // max_layers_minus1
        w.write(0, 3); // max_sub_layers_minus1
        w.write(1, 1); // temporal_id_nesting_flag
        w.write(0xFFFF, 16); // reserved 16 bits

        // profile_tier_level
        w.write(0, 2); // general_profile_space
        w.write(0, 1); // general_tier_flag
        w.write(1, 5); // general_profile_idc
        w.write(0b01000000_00000000_00000000_00000000u64, 32);
        w.write(0b1011, 4);
        w.write(0, 44);
        w.write(120, 8); // general_level_idc

        // sub_layer_ordering_info_present_flag = 0
        w.write(0, 1);
        w.write_ue(1); // max_dec_pic_buffering
        w.write_ue(0); // max_num_reorder_pics
        w.write_ue(0); // max_latency_increase

        let data = w.finish();
        let mut reader = BitReader::new(&data);
        let vps = Vps::parse(&mut reader).expect("VPS parse should succeed");

        assert_eq!(vps._vps_video_parameter_set_id, 0);
        assert!(vps._vps_base_layer_internal_flag);
        assert!(vps._vps_base_layer_available_flag);
        assert_eq!(vps._max_layers_minus1, 0);
        assert_eq!(vps.max_sub_layers_minus1, 0);
        assert!(vps._temporal_id_nesting_flag);
        assert_eq!(vps._profile_tier_level.general_profile_idc, 1);
        assert_eq!(vps._profile_tier_level.general_level_idc, 120);
    }

    #[test]
    fn parse_synthetic_pps() {
        let mut w = BitWriter::new();

        w.write_ue(0); // pps_pic_parameter_set_id
        w.write_ue(0); // pps_seq_parameter_set_id
        w.write(0, 1); // dependent_slice_segments_enabled_flag
        w.write(0, 1); // output_flag_present_flag
        w.write(0, 3); // num_extra_slice_header_bits
        w.write(1, 1); // sign_data_hiding_enabled_flag
        w.write(1, 1); // cabac_init_present_flag
        w.write_ue(0); // num_ref_idx_l0_default_active_minus1
        w.write_ue(0); // num_ref_idx_l1_default_active_minus1
        w.write_se(0); // init_qp_minus26
        w.write(0, 1); // constrained_intra_pred_flag
        w.write(0, 1); // transform_skip_enabled_flag
        w.write(0, 1); // cu_qp_delta_enabled_flag = 0 (no diff_cu_qp_delta_depth)
        w.write_se(0); // pps_cb_qp_offset
        w.write_se(0); // pps_cr_qp_offset
        w.write(0, 1); // pps_slice_chroma_qp_offsets_present_flag
        w.write(0, 1); // weighted_pred_flag
        w.write(0, 1); // weighted_bipred_flag
        w.write(0, 1); // transquant_bypass_enabled_flag
        w.write(0, 1); // tiles_enabled_flag
        w.write(0, 1); // entropy_coding_sync_enabled_flag
        w.write(1, 1); // loop_filter_across_slices_enabled_flag
        w.write(1, 1); // deblocking_filter_control_present_flag
        w.write(0, 1); // deblocking_filter_override_enabled_flag
        w.write(0, 1); // pps_deblocking_filter_disabled_flag = 0
        w.write_se(0); // pps_beta_offset_div2
        w.write_se(0); // pps_tc_offset_div2
        w.write(0, 1); // pps_scaling_list_data_present_flag
        w.write(0, 1); // lists_modification_present_flag
        w.write_ue(0); // log2_parallel_merge_level_minus2
        w.write(0, 1); // slice_segment_header_extension_present_flag

        let data = w.finish();
        let mut reader = BitReader::new(&data);
        let pps = Pps::parse(&mut reader).expect("PPS parse should succeed");

        assert_eq!(pps._pps_pic_parameter_set_id, 0);
        assert_eq!(pps._pps_seq_parameter_set_id, 0);
        assert!(!pps._dependent_slice_segments_enabled_flag);
        assert!(pps.sign_data_hiding_enabled_flag);
        assert!(pps._cabac_init_present_flag);
        assert_eq!(pps.init_qp_minus26, 0);
        assert!(!pps._cu_qp_delta_enabled_flag);
        assert!(!pps._tiles_enabled_flag);
        assert!(pps._loop_filter_across_slices_enabled_flag);
        assert!(pps._deblocking_filter_control_present_flag);
        assert!(!pps.pps_deblocking_filter_disabled_flag);
        assert!(!pps._lists_modification_present_flag);
        assert!(!pps._slice_segment_header_extension_present_flag);
    }

    #[test]
    fn parse_pps_with_tiles() {
        let mut w = BitWriter::new();

        w.write_ue(1); // pps_pic_parameter_set_id
        w.write_ue(0); // pps_seq_parameter_set_id
        w.write(0, 1); // dependent_slice_segments_enabled_flag
        w.write(0, 1); // output_flag_present_flag
        w.write(0, 3); // num_extra_slice_header_bits
        w.write(0, 1); // sign_data_hiding_enabled_flag
        w.write(0, 1); // cabac_init_present_flag
        w.write_ue(0); // num_ref_idx_l0_default_active_minus1
        w.write_ue(0); // num_ref_idx_l1_default_active_minus1
        w.write_se(0); // init_qp_minus26
        w.write(0, 1); // constrained_intra_pred_flag
        w.write(0, 1); // transform_skip_enabled_flag
        w.write(0, 1); // cu_qp_delta_enabled_flag
        w.write_se(0); // pps_cb_qp_offset
        w.write_se(0); // pps_cr_qp_offset
        w.write(0, 1); // pps_slice_chroma_qp_offsets_present_flag
        w.write(0, 1); // weighted_pred_flag
        w.write(0, 1); // weighted_bipred_flag
        w.write(0, 1); // transquant_bypass_enabled_flag
        w.write(1, 1); // tiles_enabled_flag = 1
        w.write(0, 1); // entropy_coding_sync_enabled_flag
        // tile params
        w.write_ue(1); // num_tile_columns_minus1 = 1 (2 columns)
        w.write_ue(1); // num_tile_rows_minus1 = 1 (2 rows)
        w.write(1, 1); // uniform_spacing_flag
        w.write(1, 1); // loop_filter_across_tiles_enabled_flag
        w.write(1, 1); // loop_filter_across_slices_enabled_flag
        w.write(0, 1); // deblocking_filter_control_present_flag
        w.write(0, 1); // pps_scaling_list_data_present_flag
        w.write(0, 1); // lists_modification_present_flag
        w.write_ue(0); // log2_parallel_merge_level_minus2
        w.write(0, 1); // slice_segment_header_extension_present_flag

        let data = w.finish();
        let mut reader = BitReader::new(&data);
        let pps = Pps::parse(&mut reader).expect("PPS parse should succeed");

        assert_eq!(pps._pps_pic_parameter_set_id, 1);
        assert!(pps._tiles_enabled_flag);
        assert_eq!(pps._num_tile_columns_minus1, 1);
        assert_eq!(pps._num_tile_rows_minus1, 1);
        assert!(pps._uniform_spacing_flag);
        assert!(pps._loop_filter_across_tiles_enabled_flag);
    }

    #[test]
    fn sps_bit_depth_computed() {
        let mut w = BitWriter::new();

        w.write(0, 4); // sps_video_parameter_set_id
        w.write(0, 3); // sps_max_sub_layers_minus1
        w.write(1, 1); // sps_temporal_id_nesting_flag

        // Minimal profile_tier_level
        w.write(0, 2);
        w.write(0, 1);
        w.write(1, 5);
        w.write(0, 32);
        w.write(0, 4);
        w.write(0, 44);
        w.write(120, 8);

        w.write_ue(0); // sps_seq_parameter_set_id
        w.write_ue(1); // chroma_format_idc
        w.write_ue(3840); // width
        w.write_ue(2160); // height
        w.write(0, 1); // conformance_window_flag
        w.write_ue(2); // bit_depth_luma_minus8 = 2 => 10-bit
        w.write_ue(2); // bit_depth_chroma_minus8 = 2 => 10-bit
        w.write_ue(4); // log2_max_pic_order_cnt_lsb_minus4
        w.write(0, 1); // sps_sub_layer_ordering_info_present_flag
        w.write_ue(1);
        w.write_ue(0);
        w.write_ue(0); // ordering info
        w.write_ue(0);
        w.write_ue(3); // coding block sizes
        w.write_ue(0);
        w.write_ue(3); // transform block sizes
        w.write_ue(1);
        w.write_ue(1); // hierarchy depths
        w.write(0, 1); // scaling_list_enabled_flag
        w.write(1, 1); // amp_enabled_flag
        w.write(1, 1); // sample_adaptive_offset_enabled_flag
        w.write(0, 1); // pcm_enabled_flag
        w.write_ue(0); // num_short_term_ref_pic_sets
        w.write(0, 1); // long_term_ref_pics_present_flag
        w.write(1, 1); // sps_temporal_mvp_enabled_flag
        w.write(1, 1); // strong_intra_smoothing_enabled_flag
        w.write(0, 1); // vui_parameters_present_flag

        let data = w.finish();
        let mut reader = BitReader::new(&data);
        let sps = Sps::parse(&mut reader, 0).unwrap();

        assert_eq!(sps.bit_depth_luma, 10);
        assert_eq!(sps.bit_depth_chroma, 10);
        assert_eq!(sps.pic_width_in_luma_samples, 3840);
        assert_eq!(sps.pic_height_in_luma_samples, 2160);
    }
}
