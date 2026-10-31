//! FFmpeg's hardware encoders (Windows, Linux) hand over Annex B: NAL units after `00 00 01` or `00 00 00 01` start
//! codes, with the parameter sets in-band before keyframes and in the extradata. Our files carry AVCC: each unit after
//! its 4-byte length, and the parameter sets only in the sample entry's `avcC` or `hvcC` (`StreamInfo::config`). This
//! turns one into the other.
//!
//! - **Emulation-prevention bytes stay** (both forms keep them); only the SPS fields the record repeats are read, from
//!   an unescaped copy.
//! - **Units end on their last non-zero byte.** A 4-byte start code's leading zero and an encoder's `trailing_zero_8bits`
//!   belong to no unit, and recovery's check of a sample (`recover::nal`) refuses a unit ending in a zero.
//! - **The codec is an argument.** An H.264 PPS with `nal_ref_idc` 1 (`0x28`, as VideoToolbox writes it) reads as an
//!   HEVC IDR_N_LP header (`0x28 0x01`), so the unit types can't be told apart from the bytes alone.
//!
//! `to_avcc` is O(packet bytes) with one allocation (the packet's own, as `EncodedPacket` holds a `Vec`).

use crate::media::Codec;

/// The NAL units of an Annex B stream, without their start codes or trailing zeros. Bytes before the first start code
/// belong to no unit and are skipped.
pub fn nal_units(annex_b: &[u8]) -> NalUnits<'_> {
    NalUnits {
        data: annex_b,
        at: 0,
    }
}

pub struct NalUnits<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Iterator for NalUnits<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        loop {
            let (_, begin) = start_code(self.data, self.at)?;
            let end = start_code(self.data, begin).map_or(self.data.len(), |(at, _)| at);
            self.at = end;
            let mut unit = &self.data[begin..end];
            while let [rest @ .., 0] = unit {
                unit = rest;
            }
            if !unit.is_empty() {
                return Some(unit);
            }
        }
    }
}

/// The next `00 00 01` at or after `from`: where it starts and where the unit after it starts.
fn start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    while i + 2 < data.len() {
        match data[i + 2] {
            // No start code can begin at i, i + 1 or i + 2.
            2.. => i += 3,
            1 if data[i] == 0 && data[i + 1] == 0 => return Some((i, i + 3)),
            _ => i += 1,
        }
    }
    None
}

fn unit_type(codec: Codec, unit: &[u8]) -> u8 {
    match codec {
        Codec::H264 => unit[0] & 0x1F,
        Codec::Hevc => (unit[0] >> 1) & 0x3F,
    }
}

/// Where a parameter set goes in `config_record`'s lists: H.264 SPS, PPS; HEVC VPS, SPS, PPS.
fn parameter_set(codec: Codec, unit: &[u8]) -> Option<usize> {
    match (codec, unit_type(codec, unit)) {
        (Codec::H264, 7) | (Codec::Hevc, 32) => Some(0),
        (Codec::H264, 8) | (Codec::Hevc, 33) => Some(1),
        (Codec::Hevc, 34) => Some(2),
        _ => None,
    }
}

/// One encoded frame from Annex B to AVCC: start codes become 4-byte lengths and the parameter sets are kept out (the
/// sample entry has them). Every other unit (slices, SEI, delimiters) is kept as it is. No start code gives nothing.
pub fn to_avcc(codec: Codec, annex_b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(annex_b.len() + 16);
    for unit in nal_units(annex_b).filter(|u| parameter_set(codec, u).is_none()) {
        out.extend_from_slice(&(unit.len() as u32).to_be_bytes());
        out.extend_from_slice(unit);
    }
    out
}

/// The `avcC` or `hvcC` record's body from the SPS and PPS (and the VPS) in an encoder's extradata or first keyframe:
/// version 1, the profile, compatibility and level from the first SPS, 4-byte lengths, each distinct set once. None when
/// a set is missing or an SPS doesn't parse.
pub fn config_record(codec: Codec, annex_b_extradata: &[u8]) -> Option<Vec<u8>> {
    let mut sets: [Vec<&[u8]>; 3] = Default::default();
    for unit in nal_units(annex_b_extradata) {
        if let Some(slot) = parameter_set(codec, unit)
            && !sets[slot].contains(&unit)
        {
            sets[slot].push(unit);
        }
    }
    match codec {
        Codec::H264 => avcc(&sets[0], &sets[1]),
        Codec::Hevc => hvcc(&sets[0], &sets[1], &sets[2]),
    }
}

/// ISO 14496-15 section 5.3.3, with the chroma and bit-depth fields the High profiles add.
fn avcc(sps: &[&[u8]], pps: &[&[u8]]) -> Option<Vec<u8>> {
    let first = sps.first()?;
    if pps.is_empty() || sps.len() > 31 || pps.len() > 255 {
        return None;
    }
    let rbsp = unescape(first.get(1..)?);
    let (profile, compatibility, level) = (*rbsp.first()?, *rbsp.get(1)?, *rbsp.get(2)?);
    let mut out = vec![
        1,
        profile,
        compatibility,
        level,
        0xFC | 3,
        0xE0 | sps.len() as u8,
    ];
    for unit in sps {
        push_unit(&mut out, unit)?;
    }
    out.push(pps.len() as u8);
    for unit in pps {
        push_unit(&mut out, unit)?;
    }
    if matches!(profile, 100 | 110 | 122 | 144) {
        let mut bits = Bits::new(rbsp.get(3..)?);
        bits.ue()?; // seq_parameter_set_id
        let chroma = bits.ue()?;
        if chroma == 3 {
            bits.skip(1)?; // separate_colour_plane_flag
        }
        let (luma_depth, chroma_depth) = (bits.ue()?, bits.ue()?);
        if chroma > 3 || luma_depth > 7 || chroma_depth > 7 {
            return None;
        }
        // No SPS extensions.
        out.extend([
            0xFC | chroma as u8,
            0xF8 | luma_depth as u8,
            0xF8 | chroma_depth as u8,
            0,
        ]);
    }
    Some(out)
}

/// ISO 14496-15 section 8.3.3, for `hvc1`: every parameter set is in the record (array_completeness 1). The fields that need
/// the VUI or PPS (min_spatial_segmentation_idc, parallelismType) and the frame rates are 0, "unknown", as allowed.
fn hvcc(vps: &[&[u8]], sps: &[&[u8]], pps: &[&[u8]]) -> Option<Vec<u8>> {
    let first = sps.first()?;
    if vps.is_empty() || pps.is_empty() {
        return None;
    }
    let rbsp = unescape(first.get(2..)?);
    let mut bits = Bits::new(&rbsp);
    bits.skip(4)?; // sps_video_parameter_set_id
    let sub_layers = bits.bits(3)?; // sps_max_sub_layers_minus1
    let nested = bits.bits(1)?; // sps_temporal_id_nesting_flag
    // The general profile_tier_level is byte-aligned: profile space, tier and profile, 32 compatibility flags, 48
    // constraint flags, the level. The record repeats it as it is.
    let general = rbsp.get(1..13)?;
    bits.skip(96)?;
    if sub_layers > 6 {
        return None;
    }
    let mut present = [(false, false); 6];
    for p in present.iter_mut().take(sub_layers as usize) {
        *p = (bits.bits(1)? == 1, bits.bits(1)? == 1);
    }
    if sub_layers > 0 {
        bits.skip(2 * (8 - sub_layers))?; // reserved_zero_2bits
    }
    for &(profile, level) in present.iter().take(sub_layers as usize) {
        bits.skip(88 * u32::from(profile) + 8 * u32::from(level))?;
    }
    bits.ue()?; // sps_seq_parameter_set_id
    let chroma = bits.ue()?;
    if chroma == 3 {
        bits.skip(1)?; // separate_colour_plane_flag
    }
    bits.ue()?; // pic_width_in_luma_samples
    bits.ue()?; // pic_height_in_luma_samples
    if bits.bits(1)? == 1 {
        for _ in 0..4 {
            bits.ue()?; // conformance window offsets
        }
    }
    let (luma_depth, chroma_depth) = (bits.ue()?, bits.ue()?);
    if chroma > 3 || luma_depth > 7 || chroma_depth > 7 {
        return None;
    }
    let mut out = Vec::with_capacity(
        23 + [vps, sps, pps]
            .iter()
            .flat_map(|s| s.iter())
            .map(|u| 5 + u.len())
            .sum::<usize>(),
    );
    out.push(1);
    out.extend_from_slice(general);
    out.extend([0xF0, 0x00, 0xFC]); // min_spatial_segmentation_idc 0; parallelismType 0
    out.extend([
        0xFC | chroma as u8,
        0xF8 | luma_depth as u8,
        0xF8 | chroma_depth as u8,
    ]);
    out.extend([0, 0]); // avgFrameRate
    // constantFrameRate 0, numTemporalLayers, temporalIdNested, lengthSizeMinusOne 3.
    out.push(((sub_layers as u8 + 1) << 3) | ((nested as u8) << 2) | 3);
    out.push(3);
    for (kind, units) in [(32u8, vps), (33, sps), (34, pps)] {
        out.push(0x80 | kind);
        out.extend(u16::try_from(units.len()).ok()?.to_be_bytes());
        for unit in units {
            push_unit(&mut out, unit)?;
        }
    }
    Some(out)
}

fn push_unit(out: &mut Vec<u8>, unit: &[u8]) -> Option<()> {
    out.extend(u16::try_from(unit.len()).ok()?.to_be_bytes());
    out.extend_from_slice(unit);
    Some(())
}

/// The unit's payload without its emulation-prevention bytes (the `03` in `00 00 03`).
fn unescape(escaped: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(escaped.len());
    let mut zeros = 0;
    for &b in escaped {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// A big-endian bit reader with exp-Golomb codes; None past the end.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn skip(&mut self, n: u32) -> Option<()> {
        let at = self.at + n as usize;
        (at <= self.data.len() * 8).then(|| self.at = at)
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        (0..n).try_fold(0u32, |v, _| {
            let byte = *self.data.get(self.at / 8)?;
            let bit = (byte >> (7 - self.at % 8)) & 1;
            self.at += 1;
            Some((v << 1) | u32::from(bit))
        })
    }

    /// ue(v): leading zeros, a one, then as many bits.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bits(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.bits(zeros)?)
    }
}
