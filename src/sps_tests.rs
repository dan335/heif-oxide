//! SPS VUI reader tests: real x265 parameter sets from the committed
//! fixtures, plus hand-assembled SPSs that exercise the optional syntax
//! (sub-layers, scaling lists, PCM, predicted reference picture sets) that
//! sits between the start of the SPS and the VUI.

use super::*;
use crate::test_builder as tb;

/// MSB-first bit writer producing an SPS NAL unit (header + RBSP with
/// emulation prevention), independent of the reader under test.
struct SpsWriter {
    bits: Vec<bool>,
}

impl SpsWriter {
    fn new() -> Self {
        SpsWriter { bits: Vec::new() }
    }

    fn u(&mut self, n: u32, value: u32) -> &mut Self {
        for i in (0..n).rev() {
            self.bits.push((value >> i) & 1 == 1);
        }
        self
    }

    fn flag(&mut self, value: bool) -> &mut Self {
        self.bits.push(value);
        self
    }

    fn ue(&mut self, value: u32) -> &mut Self {
        let code = value as u64 + 1;
        let len = 64 - code.leading_zeros();
        for _ in 0..len - 1 {
            self.bits.push(false);
        }
        for i in (0..len).rev() {
            self.bits.push((code >> i) & 1 == 1);
        }
        self
    }

    fn se(&mut self, value: i32) -> &mut Self {
        let mapped = if value > 0 {
            2 * value as u32 - 1
        } else {
            2 * value.unsigned_abs()
        };
        self.ue(mapped)
    }

    /// Zero bits (profile/level payloads whose content is irrelevant).
    fn zeros(&mut self, n: usize) -> &mut Self {
        self.bits.extend(std::iter::repeat_n(false, n));
        self
    }

    /// Finish with rbsp_trailing_bits and wrap as a NAL unit.
    fn finish(&self) -> Vec<u8> {
        let mut bits = self.bits.clone();
        bits.push(true);
        while !bits.len().is_multiple_of(8) {
            bits.push(false);
        }
        let rbsp: Vec<u8> = bits
            .chunks(8)
            .map(|byte| byte.iter().fold(0u8, |acc, &b| (acc << 1) | b as u8))
            .collect();
        // NAL header: type 33 (SPS), layer 0, temporal id 0 (+1).
        let mut nal = vec![33 << 1, 1];
        let mut zeros = 0;
        for byte in rbsp {
            if zeros >= 2 && byte <= 3 {
                nal.push(3); // emulation_prevention_three_byte
                zeros = 0;
            }
            zeros = if byte == 0 { zeros + 1 } else { 0 };
            nal.push(byte);
        }
        nal
    }
}

/// What varies between the hand-assembled SPSs.
#[derive(Default)]
struct SpsOptions {
    /// sps_max_sub_layers_minus1, with every sub-layer profile and level
    /// present.
    max_sub_layers_minus1: u32,
    chroma_444: bool,
    conformance_window: bool,
    scaling_lists: bool,
    pcm: bool,
    /// Three short-term reference picture sets, the second and third
    /// predicted from their predecessor.
    predicted_rps: bool,
    long_term_refs: bool,
    /// `None` = no VUI at all.
    vui: Option<Vui>,
}

/// `(video_full_range_flag, colour description)`; the description is
/// `(primaries, transfer, matrix)` when present.
type Signal = (bool, Option<(u8, u8, u8)>);

#[derive(Default, Clone, Copy)]
struct Vui {
    extended_sar: bool,
    overscan: bool,
    /// `None` = video_signal_type_present_flag is 0.
    signal: Option<Signal>,
}

fn build_sps(o: &SpsOptions) -> Vec<u8> {
    let mut w = SpsWriter::new();
    w.u(4, 0).u(3, o.max_sub_layers_minus1).flag(true);
    // profile_tier_level
    w.zeros(88).u(8, 93);
    for _ in 0..o.max_sub_layers_minus1 {
        w.flag(true).flag(true);
    }
    if o.max_sub_layers_minus1 > 0 {
        w.zeros(2 * (8 - o.max_sub_layers_minus1 as usize));
    }
    for _ in 0..o.max_sub_layers_minus1 {
        w.zeros(88).u(8, 90);
    }
    w.ue(0); // sps_seq_parameter_set_id
    if o.chroma_444 {
        w.ue(3).flag(false);
    } else {
        w.ue(1);
    }
    w.ue(4032).ue(3024);
    w.flag(o.conformance_window);
    if o.conformance_window {
        w.ue(0).ue(4).ue(0).ue(8);
    }
    w.ue(0).ue(0); // bit depths
    w.ue(4); // log2_max_pic_order_cnt_lsb_minus4 → 8-bit POC LSBs
    w.flag(true); // sub-layer ordering info for every sub-layer
    for _ in 0..=o.max_sub_layers_minus1 {
        w.ue(1).ue(0).ue(0);
    }
    w.ue(0).ue(3).ue(0).ue(3).ue(1).ue(1);
    w.flag(o.scaling_lists);
    if o.scaling_lists {
        w.flag(true); // sps_scaling_list_data_present_flag
        for size_id in 0..4u32 {
            let matrices = if size_id == 3 { 2 } else { 6 };
            for matrix in 0..matrices {
                if matrix % 2 == 0 {
                    // Predicted from a reference list.
                    w.flag(false).ue(0);
                } else {
                    // Explicit coefficients.
                    w.flag(true);
                    if size_id > 1 {
                        w.se(-3);
                    }
                    for i in 0..64.min(1 << (4 + (size_id << 1))) {
                        w.se(if i % 2 == 0 { 1 } else { -1 });
                    }
                }
            }
        }
    }
    w.flag(true).flag(true); // amp, sao
    w.flag(o.pcm);
    if o.pcm {
        w.u(4, 7).u(4, 7).ue(0).ue(2).flag(true);
    }
    if o.predicted_rps {
        w.ue(3);
        // Set 0: explicit, two negative pictures and one positive.
        w.ue(2).ue(1);
        w.ue(0).flag(true).ue(1).flag(false).ue(0).flag(true);
        // Set 1: predicted from set 0 (3 pictures → 4 entries). The second
        // entry is dropped (used = 0, use_delta = 0), leaving 3.
        w.flag(true).flag(false).ue(0);
        w.flag(true).flag(false).flag(false).flag(true).flag(true);
        // Set 2: predicted from set 1 (3 pictures → 4 entries). The first
        // is unused but kept (used = 0, use_delta = 1).
        w.flag(true).flag(true).ue(1);
        w.flag(false).flag(true).flag(true).flag(true).flag(true);
    } else {
        w.ue(0);
    }
    w.flag(o.long_term_refs);
    if o.long_term_refs {
        w.ue(2);
        w.u(8, 0x5A).flag(true).u(8, 0xA5).flag(false);
    }
    w.flag(true).flag(true); // temporal mvp, strong intra smoothing
    w.flag(o.vui.is_some());
    if let Some(vui) = o.vui {
        w.flag(true); // aspect_ratio_info_present_flag
        if vui.extended_sar {
            w.u(8, 255).u(16, 4).u(16, 3);
        } else {
            w.u(8, 1);
        }
        w.flag(vui.overscan);
        if vui.overscan {
            w.flag(true);
        }
        w.flag(vui.signal.is_some());
        if let Some((full_range, description)) = vui.signal {
            w.u(3, 5).flag(full_range).flag(description.is_some());
            if let Some((p, t, m)) = description {
                w.u(8, p.into()).u(8, t.into()).u(8, m.into());
            }
        }
        // Rest of the VUI: chroma_loc, neutral_chroma, field_seq,
        // frame_field_info, default_display_window, timing_info,
        // bitstream_restriction — all absent.
        w.zeros(7);
    }
    w.flag(false); // sps_extension_present_flag
    w.finish()
}

/// What an iPhone writes: full range, Display P3 primaries, BT.709
/// transfer, BT.601 matrix.
const IPHONE: Signal = (true, Some((12, 1, 6)));

fn iphone_vui() -> Vui {
    Vui {
        signal: Some(IPHONE),
        ..Vui::default()
    }
}

fn iphone_colour() -> VuiColour {
    VuiColour {
        full_range: true,
        description: Some((12, 1, 6)),
    }
}

#[test]
fn reads_signal_type_from_plain_sps() {
    let sps = build_sps(&SpsOptions {
        vui: Some(iphone_vui()),
        ..SpsOptions::default()
    });
    assert!(is_sps(&sps));
    assert_eq!(vui_colour(&sps), Some(iphone_colour()));
}

#[test]
fn full_range_without_colour_description() {
    let sps = build_sps(&SpsOptions {
        vui: Some(Vui {
            signal: Some((true, None)),
            ..Vui::default()
        }),
        ..SpsOptions::default()
    });
    assert_eq!(
        vui_colour(&sps),
        Some(VuiColour {
            full_range: true,
            description: None
        })
    );
}

#[test]
fn limited_range_is_reported_as_such() {
    let sps = build_sps(&SpsOptions {
        vui: Some(Vui {
            extended_sar: true,
            overscan: true,
            signal: Some((false, Some((1, 1, 1)))),
        }),
        ..SpsOptions::default()
    });
    assert_eq!(
        vui_colour(&sps),
        Some(VuiColour {
            full_range: false,
            description: Some((1, 1, 1))
        })
    );
}

#[test]
fn no_vui_or_no_signal_type_is_none() {
    let sps = build_sps(&SpsOptions::default());
    assert_eq!(vui_colour(&sps), None);
    let sps = build_sps(&SpsOptions {
        vui: Some(Vui::default()),
        ..SpsOptions::default()
    });
    assert_eq!(vui_colour(&sps), None);
}

/// Every optional structure ahead of the VUI is skipped by exactly its
/// coded length — one bit off and the colour fields would read as garbage.
#[test]
fn skips_optional_syntax_before_the_vui() {
    let variants = [
        SpsOptions {
            max_sub_layers_minus1: 2,
            ..SpsOptions::default()
        },
        SpsOptions {
            chroma_444: true,
            conformance_window: true,
            ..SpsOptions::default()
        },
        SpsOptions {
            scaling_lists: true,
            ..SpsOptions::default()
        },
        SpsOptions {
            pcm: true,
            ..SpsOptions::default()
        },
        SpsOptions {
            predicted_rps: true,
            ..SpsOptions::default()
        },
        SpsOptions {
            long_term_refs: true,
            ..SpsOptions::default()
        },
        SpsOptions {
            max_sub_layers_minus1: 6,
            chroma_444: true,
            conformance_window: true,
            scaling_lists: true,
            pcm: true,
            predicted_rps: true,
            long_term_refs: true,
            vui: None,
        },
    ];
    for (i, mut options) in variants.into_iter().enumerate() {
        options.vui = Some(Vui {
            extended_sar: i % 2 == 0,
            overscan: i % 3 == 0,
            signal: Some(IPHONE),
        });
        assert_eq!(
            vui_colour(&build_sps(&options)),
            Some(iphone_colour()),
            "variant {i}"
        );
    }
}

/// Truncating a valid SPS anywhere, or feeding arbitrary bytes, never
/// panics — it yields `None` (or, once past the colour fields, the value).
#[test]
fn truncated_and_garbage_input_is_harmless() {
    let sps = build_sps(&SpsOptions {
        max_sub_layers_minus1: 3,
        scaling_lists: true,
        predicted_rps: true,
        long_term_refs: true,
        vui: Some(iphone_vui()),
        ..SpsOptions::default()
    });
    for len in 0..sps.len() {
        let got = vui_colour(&sps[..len]);
        assert!(got.is_none() || got == Some(iphone_colour()), "len {len}");
    }
    // Deterministic pseudo-random payloads behind a valid SPS header.
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    for _ in 0..2000 {
        let mut nal = vec![33 << 1, 1];
        for _ in 0..(state % 96) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            nal.push(state as u8);
        }
        let _ = vui_colour(&nal);
    }
    // All-zero and all-one payloads (longest Exp-Golomb prefixes).
    let _ = vui_colour(&[&[33 << 1, 1][..], &[0u8; 64][..]].concat());
    let _ = vui_colour(&[&[33 << 1, 1][..], &[0xFFu8; 64][..]].concat());
}

/// Only base-layer SPS NAL units are parsed.
#[test]
fn other_nal_types_are_ignored() {
    let mut nal = build_sps(&SpsOptions {
        vui: Some(iphone_vui()),
        ..SpsOptions::default()
    });
    nal[0] = 34 << 1; // PPS
    assert!(!is_sps(&nal));
    assert_eq!(vui_colour(&nal), None);
    nal[0] = 33 << 1;
    nal[1] = (1 << 3) | 1; // nuh_layer_id 1
    assert!(!is_sps(&nal));
    assert!(!is_sps(&[]));
}

/// The committed fixtures carry real x265 parameter sets: the reader gets
/// through each SPS and reports what the encoder was told to signal.
#[test]
fn parses_real_x265_parameter_sets() {
    let sps_colour = |name: &str| {
        let path = format!("{}/testdata/{name}.h265", env!("CARGO_MANIFEST_DIR"));
        let annex_b = std::fs::read(path).expect("read fixture");
        let sps = tb::split_annex_b(&annex_b)
            .into_iter()
            .find(|nal| is_sps(nal))
            .unwrap_or_else(|| panic!("{name}: no SPS"))
            .to_vec();
        vui_colour(&sps)
    };
    // Authored without range or colour options.
    for name in [
        "flat_red_64",
        "flat_blue_64",
        "flat_green_64",
        "flat_gray_64",
        "flat_mid_64_10bit",
    ] {
        let colour = sps_colour(name);
        assert!(colour.is_none_or(|c| !c.full_range), "{name}: {colour:?}");
    }
    // Authored the way an iPhone signals its pictures.
    assert_eq!(sps_colour("flat_full_range_64"), Some(iphone_colour()));
}
