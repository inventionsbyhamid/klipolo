//! Synthetic encoded video. Each packet is one AVCC NAL unit that carries its frame number and whether it is a flash
//! frame (the sync schedule's picture half):
//! `[length u32 BE][NAL header][frame u64 BE][flash u8][zeros up to packet_bytes]`. The NAL header is H.264's IDR
//! (0x65) or non-IDR slice (0x41), or HEVC's IDR_W_RADL (0x26 0x01) or TRAIL_R (0x02 0x01). The packets don't decode;
//! `fixture` gives real packets for that.

use crate::media::{Codec, EncodedPacket, StreamInfo};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntheticVideoSpec {
    pub fps: u32,
    /// A keyframe every this many frames.
    pub gop_frames: u32,
    pub width: u32,
    pub height: u32,
    pub codec: Codec,
    /// Frames whose number is a multiple of this are flash frames (the sync matrix's picture marks).
    pub flash_every: Option<u32>,
    /// Each packet's size, length prefix included (at least the header's).
    pub packet_bytes: usize,
}

/// Frames `first_frame..first_frame + count`: a keyframe every `gop_frames`, frame `f` captured at
/// `start_ns + f × 10⁹ / fps` on the host clock.
pub fn synthetic_packets(
    spec: &SyntheticVideoSpec,
    first_frame: u64,
    count: u64,
    start_ns: u64,
) -> Vec<EncodedPacket> {
    let gop = u64::from(spec.gop_frames.max(1));
    (first_frame..first_frame + count)
        .map(|f| synthetic_packet(spec, f, f.is_multiple_of(gop), frame_ns(spec, f, start_ns)))
        .collect()
}

/// When frame `frame` is captured: `start_ns + frame × 10⁹ / fps`.
pub fn frame_ns(spec: &SyntheticVideoSpec, frame: u64, start_ns: u64) -> u64 {
    start_ns + (u128::from(frame) * 1_000_000_000 / u128::from(spec.fps.max(1))) as u64
}

/// One frame's packet.
pub fn synthetic_packet(
    spec: &SyntheticVideoSpec,
    frame: u64,
    keyframe: bool,
    host_ns: u64,
) -> EncodedPacket {
    let header: &[u8] = match (spec.codec, keyframe) {
        (Codec::H264, true) => &[0x65],
        (Codec::H264, false) => &[0x41],
        (Codec::Hevc, true) => &[0x26, 0x01],
        (Codec::Hevc, false) => &[0x02, 0x01],
    };
    let at = 4 + header.len();
    let mut data = vec![0u8; spec.packet_bytes.max(at + 9)];
    let nal_len = (data.len() - 4) as u32;
    data[..4].copy_from_slice(&nal_len.to_be_bytes());
    data[4..at].copy_from_slice(header);
    data[at..at + 8].copy_from_slice(&frame.to_be_bytes());
    data[at + 8] = u8::from(is_flash(spec, frame));
    EncodedPacket {
        host_ns,
        keyframe,
        data,
    }
}

fn is_flash(spec: &SyntheticVideoSpec, frame: u64) -> bool {
    spec.flash_every
        .is_some_and(|n| n > 0 && frame.is_multiple_of(u64::from(n)))
}

/// A synthetic packet's frame number and flash flag; None for any other packet.
pub fn frame_of(packet: &EncodedPacket) -> Option<(u64, bool)> {
    let d = &packet.data;
    let at = match d.get(4..6)? {
        [0x65 | 0x41, _] => 5,
        [0x26 | 0x02, 0x01] => 6,
        _ => return None,
    };
    let nal_len = u32::from_be_bytes(d[..4].try_into().ok()?) as usize;
    if nal_len != d.len() - 4 {
        return None;
    }
    let frame = u64::from_be_bytes(d.get(at..at + 8)?.try_into().ok()?);
    Some((frame, *d.get(at + 8)? != 0))
}

/// The stream's description: the spec's size, with a well-formed config record of a 320×180 stream (H.264 Constrained
/// Baseline 1.3, or HEVC Main 2.0), constant bytes taken from libx264's and libx265's output for a generated test
/// pattern.
pub fn synthetic_info(spec: &SyntheticVideoSpec) -> StreamInfo {
    let config = match spec.codec {
        Codec::H264 => AVCC.to_vec(),
        Codec::Hevc => HVCC.to_vec(),
    };
    StreamInfo {
        codec: spec.codec,
        config,
        width: spec.width,
        height: spec.height,
    }
}

/// `avcC`: version 1, profile 66 (constraints 0xc0), level 13, 4-byte lengths, one SPS (25 bytes), one PPS (5).
#[rustfmt::skip]
const AVCC: [u8; 41] = [
    0x01, 0x42, 0xc0, 0x0d, 0xff, 0xe1, 0x00, 0x19, 0x67, 0x42, 0xc0, 0x0d, 0xd9, 0x01, 0x41, 0x9f,
    0x9f, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03, 0x03, 0xc0, 0xf1, 0x42, 0xa4,
    0x80, 0x01, 0x00, 0x05, 0x68, 0xcb, 0x83, 0xcb, 0x20,
];

/// `hvcC`: version 1, Main, level 60, 4-byte lengths, three complete arrays: a VPS (24 bytes), an SPS (43), a PPS (7).
#[rustfmt::skip]
const HVCC: [u8; 112] = [
    0x01, 0x01, 0x60, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3c, 0xf0, 0x00, 0xfc,
    0xfd, 0xf8, 0xf8, 0x00, 0x00, 0x0f, 0x03, 0xa0, 0x00, 0x01, 0x00, 0x18, 0x40, 0x01, 0x0c, 0x01,
    0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00,
    0x3c, 0x95, 0x98, 0x09, 0xa1, 0x00, 0x01, 0x00, 0x2b, 0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00,
    0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3c, 0xa0, 0x0a, 0x08, 0x0b, 0x9f,
    0x79, 0x65, 0x66, 0x92, 0x4c, 0xaf, 0x01, 0x68, 0x08, 0x00, 0x00, 0x03, 0x00, 0x08, 0x00, 0x00,
    0x03, 0x00, 0xf0, 0x40, 0xa2, 0x00, 0x01, 0x00, 0x07, 0x44, 0x01, 0xc1, 0x72, 0xb4, 0x62, 0x40,
];
