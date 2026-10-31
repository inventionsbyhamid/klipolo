//! FFmpeg's hardware encoders (Windows, Linux) give Annex B. The fixtures' frames as such an encoder hands them over,
//! turned into AVCC and a config record, must make a file that decodes frame for frame.
#![cfg(feature = "synthetic")]
mod common;
use std::path::{Path, PathBuf};

use common::decode::decode_all;
use klipolo_capture::annexb::{config_record, nal_units, to_avcc};
use klipolo_capture::fmp4::Fmp4Writer;
use klipolo_capture::media::{Codec, EncodedPacket, StreamInfo};
use klipolo_capture::synthetic::FixtureVideo;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The stream cut into what an encoder hands over per frame (these fixtures have one slice a picture): the units up to
/// and including each picture's slice, behind 4-byte start codes, and whether it is a keyframe.
fn access_units(stream: &[u8], codec: Codec) -> Vec<(bool, Vec<u8>)> {
    let (mut frames, mut frame) = (vec![], vec![]);
    for unit in nal_units(stream) {
        frame.extend_from_slice(&[0, 0, 0, 1]);
        frame.extend_from_slice(unit);
        let (picture, key) = match codec {
            Codec::H264 => (matches!(unit[0] & 0x1F, 1..=5), unit[0] & 0x1F == 5),
            Codec::Hevc => {
                let kind = (unit[0] >> 1) & 0x3F;
                (kind < 32, (16..=23).contains(&kind))
            }
        };
        if picture {
            frames.push((key, std::mem::take(&mut frame)));
        }
    }
    frames
}

fn round_trip(name: &str, codec: Codec) {
    let stream = std::fs::read(fixtures().join(format!("{name}.annexb"))).unwrap();
    let mp4 = FixtureVideo::load(&fixtures().join(format!("{name}.mp4")));
    let frames = access_units(&stream, codec);
    assert_eq!(frames.len(), 60);
    let config = config_record(codec, &frames[0].1).expect("the keyframe's parameter sets");
    // FFmpeg's own record for the same stream: the same bytes for H.264. For HEVC the fixed part (profile, tier, level,
    // chroma, bit depths, layers, length size) matches; FFmpeg's arrays also list the encoder's SEI.
    match codec {
        Codec::H264 => assert_eq!(config, mp4.info.config),
        Codec::Hevc => assert_eq!(config[..22], mp4.info.config[..22]),
    }
    let info = StreamInfo {
        codec,
        config,
        width: 320,
        height: 180,
    };
    let mut writer = Fmp4Writer::new(&info);
    let mut file = writer.head();
    for (i, (keyframe, annex_b)) in frames.iter().enumerate() {
        let data = to_avcc(codec, annex_b);
        // The MP4's own sample (HEVC's conversion also repeats the encoder's SEI before each keyframe).
        assert!(
            *keyframe == mp4.frames[i].0 && data.ends_with(&mp4.frames[i].1),
            "frame {i}"
        );
        let packet = EncodedPacket {
            host_ns: i as u64 * 1_000_000_000 / 30,
            keyframe: *keyframe,
            data,
        };
        if let Some(fragment) = writer.push(i as i64 * 1_000_000 / 30, &packet) {
            file.extend(fragment.bytes);
        }
    }
    if let Some(fragment) = writer.flush() {
        file.extend(fragment.bytes);
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{name}.mp4"));
    std::fs::write(&path, &file).unwrap();
    assert_eq!(decode_all(&path).0, 60);
}

#[test]
fn h264_annex_b_becomes_a_file_that_decodes_frame_for_frame() {
    round_trip("gop-h264", Codec::H264);
}

#[test]
fn hevc_annex_b_becomes_a_file_that_decodes_frame_for_frame() {
    round_trip("gop-hevc", Codec::Hevc);
}
