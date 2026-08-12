//! HEVC glue: `hvcC` configuration parsing and item codestream decoding.
//!
//! A HEIF `hvc1` item stores its VPS/SPS/PPS parameter sets inside the
//! `hvcC` property (HEVCDecoderConfigurationRecord, ISO/IEC 14496-15 §8.3.3)
//! and its slice NAL units as a **length-prefixed** stream in `mdat`. The
//! `rust_h265` decoder consumes Annex B (start-code delimited) NAL units, so
//! this module rebuilds an Annex B stream — parameter sets first, then the
//! item's NALs with each length prefix replaced by a start code — and runs
//! the decoder over it.

use rust_h265::{parse_annex_b, Decoder, Frame};

use crate::bytes::Reader;
use crate::error::HeifError;

/// The parts of an `hvcC` record the decoder needs.
pub struct HvcConfig {
    /// Byte width of each NAL length prefix in the item payload
    /// (`lengthSizeMinusOne + 1`; 1, 2 or 4).
    pub nal_length_size: u8,
    /// Parameter-set NAL units (VPS, SPS, PPS, and any prefix SEI), in
    /// record order, without start codes or length prefixes.
    pub parameter_sets: Vec<Vec<u8>>,
}

/// Parse an HEVCDecoderConfigurationRecord.
pub fn parse_hvcc(record: &[u8]) -> Result<HvcConfig, HeifError> {
    let mut r = Reader::new(record, "hvcC");
    // Fixed-layout preamble (ISO 14496-15 §8.3.3.1):
    //   [0]     configurationVersion (= 1)
    //   [1..21] profile/tier/level, constraint flags, chroma format,
    //           bit depths, frame rate — all irrelevant here, the SPS
    //           carries the authoritative values.
    //   [21]    …(2 bits)… | lengthSizeMinusOne (2 bits)
    //   [22]    numOfArrays
    let version = r.u8()?;
    if version != 1 {
        return Err(HeifError::Invalid(format!(
            "hvcC configurationVersion {version} (expected 1)"
        )));
    }
    r.skip(20)?;
    let nal_length_size = (r.u8()? & 0x3) + 1;
    let num_arrays = r.u8()?;
    let mut parameter_sets = Vec::new();
    for _ in 0..num_arrays {
        // array_completeness (1) | reserved (1) | NAL_unit_type (6)
        let _type_byte = r.u8()?;
        let num_nalus = r.u16()?;
        for _ in 0..num_nalus {
            let len = r.u16()? as usize;
            parameter_sets.push(r.bytes(len)?.to_vec());
        }
    }
    Ok(HvcConfig {
        nal_length_size,
        parameter_sets,
    })
}

/// Convert an `hvcC` config + length-prefixed item payload into one Annex B
/// stream: `startcode paramset ... startcode nal ...`.
pub fn build_annex_b(config: &HvcConfig, item_payload: &[u8]) -> Result<Vec<u8>, HeifError> {
    const START_CODE: [u8; 4] = [0, 0, 0, 1];
    let mut stream = Vec::with_capacity(
        item_payload.len()
            + config
                .parameter_sets
                .iter()
                .map(|p| p.len() + 4)
                .sum::<usize>()
            + 64,
    );
    for ps in &config.parameter_sets {
        stream.extend_from_slice(&START_CODE);
        stream.extend_from_slice(ps);
    }
    let mut r = Reader::new(item_payload, "hvc1 item payload");
    while r.remaining() > 0 {
        let len = match config.nal_length_size {
            1 => r.u8()? as usize,
            2 => r.u16()? as usize,
            4 => r.u32()? as usize,
            n => {
                return Err(HeifError::Invalid(format!(
                    "hvcC NAL length size {n} (expected 1, 2 or 4)"
                )))
            }
        };
        stream.extend_from_slice(&START_CODE);
        stream.extend_from_slice(r.bytes(len)?);
    }
    Ok(stream)
}

/// Decode a single still picture from an Annex B stream, returning the first
/// output frame (a HEIF item is exactly one coded picture).
pub fn decode_first_frame(annex_b: &[u8]) -> Result<Frame, HeifError> {
    let nals = parse_annex_b(annex_b);
    if nals.is_empty() {
        return Err(HeifError::Codec("no NAL units in item payload".into()));
    }
    let mut decoder = Decoder::new();
    for nal in &nals {
        match decoder.decode_nal(nal) {
            Ok(Some(frame)) => return Ok(frame),
            Ok(None) => {}
            Err(e) => return Err(HeifError::Codec(format!("{e:?}"))),
        }
    }
    decoder
        .flush()
        .ok_or_else(|| HeifError::Codec("decoder produced no frame".into()))
}

/// Conservatively inspect the coded dimensions declared by every SPS in an
/// `hvcC` record. This is used for allocation checks before the decoder sees
/// the slice payload.
pub fn coded_dimensions(record: &[u8]) -> Result<(u32, u32), HeifError> {
    let config = parse_hvcc(record)?;
    let mut dimensions = None;
    for parameter_set in &config.parameter_sets {
        if parameter_set.len() < 2 || (parameter_set[0] >> 1) & 0x3f != 33 {
            continue;
        }
        let candidate = sps_dimensions(parameter_set)?;
        if dimensions.is_some_and(|existing| existing != candidate) {
            return Err(HeifError::Invalid(
                "hvcC contains SPS records with differing dimensions".into(),
            ));
        }
        dimensions = Some(candidate);
    }
    dimensions.ok_or_else(|| HeifError::Invalid("hvcC contains no SPS record".into()))
}

fn sps_dimensions(nalu: &[u8]) -> Result<(u32, u32), HeifError> {
    let rbsp = remove_emulation_prevention(&nalu[2..]);
    let mut bits = BitReader::new(&rbsp);
    bits.skip(4)?;
    let max_sub_layers_minus_one = bits.read(3)? as usize;
    bits.skip(1)?;
    skip_profile_tier_level(&mut bits, max_sub_layers_minus_one)?;
    bits.read_exp_golomb()?;
    let chroma_format_idc = bits.read_exp_golomb()?;
    if chroma_format_idc > 3 {
        return Err(HeifError::Invalid(
            "HEVC SPS chroma_format_idc exceeds 3".into(),
        ));
    }
    if chroma_format_idc == 3 {
        bits.skip(1)?;
    }
    let width = bits.read_exp_golomb()?;
    let height = bits.read_exp_golomb()?;
    if width == 0 || height == 0 {
        return Err(HeifError::Invalid("HEVC SPS dimensions are zero".into()));
    }
    Ok((width, height))
}

fn skip_profile_tier_level(
    bits: &mut BitReader<'_>,
    max_sub_layers: usize,
) -> Result<(), HeifError> {
    bits.skip(96)?;
    let mut profile_present = [false; 7];
    let mut level_present = [false; 7];
    for index in 0..max_sub_layers {
        profile_present[index] = bits.read(1)? != 0;
        level_present[index] = bits.read(1)? != 0;
    }
    if max_sub_layers > 0 {
        bits.skip((8 - max_sub_layers) * 2)?;
    }
    for index in 0..max_sub_layers {
        if profile_present[index] {
            bits.skip(88)?;
        }
        if level_present[index] {
            bits.skip(8)?;
        }
    }
    Ok(())
}

fn remove_emulation_prevention(ebsp: &[u8]) -> Vec<u8> {
    let mut rbsp = Vec::with_capacity(ebsp.len());
    let mut zeroes = 0;
    for &byte in ebsp {
        if zeroes >= 2 && byte == 3 {
            zeroes = 0;
            continue;
        }
        rbsp.push(byte);
        zeroes = if byte == 0 { zeroes + 1 } else { 0 };
    }
    rbsp
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_offset: 0,
        }
    }

    fn read(&mut self, count: usize) -> Result<u32, HeifError> {
        if count > 32 || self.bit_offset.saturating_add(count) > self.data.len().saturating_mul(8) {
            return Err(HeifError::Truncated("HEVC SPS"));
        }
        let mut value = 0_u32;
        for _ in 0..count {
            let byte = self.data[self.bit_offset / 8];
            value = (value << 1) | u32::from((byte >> (7 - self.bit_offset % 8)) & 1);
            self.bit_offset += 1;
        }
        Ok(value)
    }

    fn skip(&mut self, count: usize) -> Result<(), HeifError> {
        if self.bit_offset.saturating_add(count) > self.data.len().saturating_mul(8) {
            return Err(HeifError::Truncated("HEVC SPS"));
        }
        self.bit_offset += count;
        Ok(())
    }

    fn read_exp_golomb(&mut self) -> Result<u32, HeifError> {
        let mut leading_zeroes = 0_usize;
        while self.read(1)? == 0 {
            leading_zeroes += 1;
            if leading_zeroes > 31 {
                return Err(HeifError::Invalid(
                    "HEVC SPS Exp-Golomb value exceeds u32".into(),
                ));
            }
        }
        let suffix = self.read(leading_zeroes)?;
        Ok(((1_u32 << leading_zeroes) - 1).saturating_add(suffix))
    }
}
