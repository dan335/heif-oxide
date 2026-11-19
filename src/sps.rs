//! Minimal HEVC SPS reader: just far enough to reach the VUI colour
//! signalling (`video_full_range_flag` and the colour description).
//!
//! `rust_h265` decodes the picture but does not surface these fields, and a
//! HEIF file may omit the `colr` nclx box — iPhones do, carrying an ICC
//! profile instead. The bitstream's own signalling is then the only
//! statement of sample range and matrix coefficients, and ignoring it
//! decodes full-range pictures as limited range (crushed shadows, clipped
//! highlights).
//!
//! Everything here is best-effort: a truncated, malformed or unusual SPS
//! yields `None` and the caller keeps its defaults. Nothing in this module
//! can fail a decode, and every read is bounds-checked.

/// NAL unit type of a sequence parameter set (ITU-T H.265 Table 7-1).
const NAL_SPS: u8 = 33;

/// Colour signalling from an SPS's VUI `video_signal_type` fields
/// (ITU-T H.265 §E.2.1), in CICP / H.273 code points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VuiColour {
    /// `video_full_range_flag`.
    pub full_range: bool,
    /// `(colour_primaries, transfer_characteristics, matrix_coeffs)`, when
    /// `colour_description_present_flag` is set.
    pub description: Option<(u8, u8, u8)>,
}

/// Whether `nal` (header included, no start code) is a base-layer SPS.
pub fn is_sps(nal: &[u8]) -> bool {
    // forbidden_zero_bit (1) | nal_unit_type (6) | nuh_layer_id (6) | tid (3)
    nal.len() > 2 && (nal[0] >> 1) & 0x3F == NAL_SPS && (nal[0] & 0x01) == 0 && (nal[1] >> 3) == 0
}

/// Read the VUI colour signalling from an SPS NAL unit. `None` when the
/// SPS carries no `video_signal_type` (or cannot be parsed that far).
pub fn vui_colour(sps_nal: &[u8]) -> Option<VuiColour> {
    if !is_sps(sps_nal) {
        return None;
    }
    let mut b = Bits::new(&sps_nal[2..]);

    // seq_parameter_set_rbsp(), ITU-T H.265 §7.3.2.2.
    b.skip(4)?; // sps_video_parameter_set_id
    let max_sub_layers_minus1 = b.u(3)?;
    if max_sub_layers_minus1 > 6 {
        return None;
    }
    b.skip(1)?; // sps_temporal_id_nesting_flag
    skip_profile_tier_level(&mut b, max_sub_layers_minus1)?;
    b.ue()?; // sps_seq_parameter_set_id
    let chroma_format_idc = b.ue()?;
    if chroma_format_idc > 3 {
        return None;
    }
    if chroma_format_idc == 3 {
        b.skip(1)?; // separate_colour_plane_flag
    }
    b.ue()?; // pic_width_in_luma_samples
    b.ue()?; // pic_height_in_luma_samples
    if b.flag()? {
        // conformance_window_flag: four offsets.
        for _ in 0..4 {
            b.ue()?;
        }
    }
    b.ue()?; // bit_depth_luma_minus8
    b.ue()?; // bit_depth_chroma_minus8
    let log2_max_poc_lsb = b.ue()?.checked_add(4)?;
    if log2_max_poc_lsb > 16 {
        return None;
    }
    // sps_sub_layer_ordering_info_present_flag: one triple per sub-layer
    // when set, otherwise a single triple.
    let ordering_triples = if b.flag()? {
        max_sub_layers_minus1 + 1
    } else {
        1
    };
    for _ in 0..ordering_triples * 3 {
        b.ue()?;
    }
    // log2_min_luma_coding_block_size_minus3, log2_diff_max_min_luma_coding_
    // block_size, log2_min_luma_transform_block_size_minus2, log2_diff_max_
    // min_luma_transform_block_size, max_transform_hierarchy_depth_inter,
    // max_transform_hierarchy_depth_intra.
    for _ in 0..6 {
        b.ue()?;
    }
    if b.flag()? && b.flag()? {
        // scaling_list_enabled_flag && sps_scaling_list_data_present_flag
        skip_scaling_list_data(&mut b)?;
    }
    b.skip(2)?; // amp_enabled_flag, sample_adaptive_offset_enabled_flag
    if b.flag()? {
        // pcm_enabled_flag
        b.skip(8)?; // pcm sample bit depths (luma, chroma)
        b.ue()?; // log2_min_pcm_luma_coding_block_size_minus3
        b.ue()?; // log2_diff_max_min_pcm_luma_coding_block_size
        b.skip(1)?; // pcm_loop_filter_disabled_flag
    }
    skip_short_term_ref_pic_sets(&mut b)?;
    if b.flag()? {
        // long_term_ref_pics_present_flag
        let count = b.ue()?;
        if count > 32 {
            return None;
        }
        for _ in 0..count {
            b.skip(log2_max_poc_lsb as usize + 1)?; // poc lsb + used flag
        }
    }
    b.skip(2)?; // sps_temporal_mvp_enabled_flag, strong_intra_smoothing_enabled_flag
    if !b.flag()? {
        return None; // vui_parameters_present_flag
    }

    // vui_parameters(), §E.2.1.
    if b.flag()? {
        // aspect_ratio_info_present_flag
        const EXTENDED_SAR: u32 = 255;
        if b.u(8)? == EXTENDED_SAR {
            b.skip(32)?; // sar_width, sar_height
        }
    }
    if b.flag()? {
        b.skip(1)?; // overscan_info_present_flag → overscan_appropriate_flag
    }
    if !b.flag()? {
        return None; // video_signal_type_present_flag
    }
    b.skip(3)?; // video_format
    let full_range = b.flag()?;
    let description = if b.flag()? {
        Some((b.u(8)? as u8, b.u(8)? as u8, b.u(8)? as u8))
    } else {
        None
    };
    Some(VuiColour {
        full_range,
        description,
    })
}

/// profile_tier_level(1, maxNumSubLayersMinus1), §7.3.3.
fn skip_profile_tier_level(b: &mut Bits, max_sub_layers_minus1: u32) -> Option<()> {
    // General profile space/tier/idc, compatibility flags, constraint
    // flags (88 bits), then general_level_idc (8 bits).
    b.skip(88 + 8)?;
    let mut profile_present = [false; 8];
    let mut level_present = [false; 8];
    for i in 0..max_sub_layers_minus1 as usize {
        profile_present[i] = b.flag()?;
        level_present[i] = b.flag()?;
    }
    if max_sub_layers_minus1 > 0 {
        // reserved_zero_2bits padding out to eight entries.
        b.skip(2 * (8 - max_sub_layers_minus1 as usize))?;
    }
    for i in 0..max_sub_layers_minus1 as usize {
        if profile_present[i] {
            b.skip(88)?;
        }
        if level_present[i] {
            b.skip(8)?;
        }
    }
    Some(())
}

/// scaling_list_data(), §7.3.4.
fn skip_scaling_list_data(b: &mut Bits) -> Option<()> {
    for size_id in 0..4u32 {
        let step = if size_id == 3 { 3 } else { 1 };
        for _matrix_id in (0..6).step_by(step) {
            if !b.flag()? {
                b.ue()?; // scaling_list_pred_matrix_id_delta
            } else {
                let coef_num = 64.min(1u32 << (4 + (size_id << 1)));
                if size_id > 1 {
                    b.se()?; // scaling_list_dc_coef_minus8
                }
                for _ in 0..coef_num {
                    b.se()?; // scaling_list_delta_coef
                }
            }
        }
    }
    Some(())
}

/// num_short_term_ref_pic_sets followed by that many st_ref_pic_set()
/// structures (§7.3.7). Sets can be coded as deltas against the previous
/// set, so the number of pictures in each one has to be tracked to know how
/// many flags the next one carries.
fn skip_short_term_ref_pic_sets(b: &mut Bits) -> Option<()> {
    let count = b.ue()?;
    if count > 64 {
        return None;
    }
    let mut prev_num_delta_pocs = 0u32;
    for idx in 0..count {
        // inter_ref_pic_set_prediction_flag is absent for the first set.
        let inter = idx != 0 && b.flag()?;
        let num_delta_pocs = if inter {
            b.skip(1)?; // delta_rps_sign
            b.ue()?; // abs_delta_rps_minus1
            let mut kept = 0;
            for _ in 0..=prev_num_delta_pocs {
                // used_by_curr_pic_flag; use_delta_flag follows only when
                // it is 0, and is inferred to be 1 otherwise.
                if b.flag()? || b.flag()? {
                    kept += 1;
                }
            }
            kept
        } else {
            let negative = b.ue()?;
            let positive = b.ue()?;
            if negative > 16 || positive > 16 {
                return None;
            }
            for _ in 0..negative + positive {
                b.ue()?; // delta_poc_s{0,1}_minus1
                b.skip(1)?; // used_by_curr_pic_s{0,1}_flag
            }
            negative + positive
        };
        prev_num_delta_pocs = num_delta_pocs;
    }
    Some(())
}

/// MSB-first bit reader over an RBSP. Every read returns `None` once the
/// data runs out.
struct Bits {
    data: Vec<u8>,
    /// Position in bits from the start of `data`.
    pos: usize,
}

impl Bits {
    /// Wrap a NAL payload (header already removed), dropping the
    /// emulation-prevention bytes (`00 00 03` → `00 00`, §7.4.2).
    fn new(payload: &[u8]) -> Self {
        let mut data = Vec::with_capacity(payload.len());
        let mut zeros = 0;
        for &byte in payload {
            if zeros >= 2 && byte == 3 {
                zeros = 0;
                continue;
            }
            zeros = if byte == 0 { zeros + 1 } else { 0 };
            data.push(byte);
        }
        Bits { data, pos: 0 }
    }

    fn flag(&mut self) -> Option<bool> {
        let byte = *self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(bit == 1)
    }

    /// Unsigned fixed-width read, `n` ≤ 32.
    fn u(&mut self, n: u32) -> Option<u32> {
        debug_assert!(n <= 32);
        let mut value = 0u32;
        for _ in 0..n {
            value = (value << 1) | u32::from(self.flag()?);
        }
        Some(value)
    }

    fn skip(&mut self, bits: usize) -> Option<()> {
        let end = self.pos.checked_add(bits)?;
        if end > self.data.len() * 8 {
            return None;
        }
        self.pos = end;
        Some(())
    }

    /// Unsigned Exp-Golomb code, ue(v) (§9.2).
    fn ue(&mut self) -> Option<u32> {
        let mut leading_zeros = 0;
        while !self.flag()? {
            leading_zeros += 1;
            if leading_zeros > 32 {
                return None;
            }
        }
        let suffix = if leading_zeros == 32 {
            // Only the all-zero suffix keeps the value inside u32.
            if self.u(32)? != 0 {
                return None;
            }
            return Some(u32::MAX);
        } else {
            self.u(leading_zeros)?
        };
        Some((1u32 << leading_zeros) - 1 + suffix)
    }

    /// Signed Exp-Golomb code, se(v). Only ever skipped here.
    fn se(&mut self) -> Option<()> {
        self.ue().map(|_| ())
    }
}

#[cfg(test)]
#[path = "sps_tests.rs"]
mod tests;
