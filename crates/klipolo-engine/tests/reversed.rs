//! A stretch of video written backward (`media::reversed`): its frames last first, each as long as it was, its sound
//! backward too. The source is made here, one grey level a frame, so the copy's order can be read back frame by frame.

mod support;

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use ffmpeg::format::Pixel;
use ffmpeg::format::sample::{Sample, Type as SampleType};
use ffmpeg::{ChannelLayout, Rational};
use klipolo_engine::audio::decode;
use klipolo_engine::export::codecs::encoder_name;
use klipolo_engine::export::quality::VideoCodec;
use klipolo_engine::media::reversed::{Reverse, ReverseError, write};
use klipolo_engine::vendor::cap::enc_ffmpeg::aac::AACEncoder;
use klipolo_engine::vendor::cap::enc_ffmpeg::h264::{H264Encoder, VtCodec};
use klipolo_engine::vendor::cap::media_info::{AudioInfo, VideoInfo};
use klipolo_model::time::RangeUs;

const FPS: i32 = 24;
const FRAMES: usize = 24;
const RATE: u32 = 48_000;

/// Frame `i`'s grey level.
fn level(i: usize) -> u8 {
    40 + 6 * i as u8
}

/// The encoder this computer has for a test movie: H.264 on a Mac, ProRes elsewhere.
fn test_codec() -> (VtCodec, &'static str) {
    match encoder_name(VideoCodec::H264High) {
        Some(name) => (VtCodec::H264High, name),
        None => (
            VtCodec::ProRes422Hq,
            encoder_name(VideoCodec::ProRes422Hq).expect("a ProRes encoder"),
        ),
    }
}

/// One second at 24 fps, 160 × 96, frame `i` all `level(i)`; with `sound`, silence for its first half and a loud tone
/// for its second.
fn ramp(path: &Path, sound: bool) {
    ffmpeg::init().unwrap();
    let mut output = ffmpeg::format::output_as(path, "mov").unwrap();
    let (codec, name) = test_codec();
    let info = VideoInfo {
        pixel_format: Pixel::NV12,
        width: 160,
        height: 96,
        time_base: Rational(1, 1_000_000),
        frame_rate: Rational(FPS, 1),
    };
    let mut video = H264Encoder::builder(info)
        .with_codec(codec)
        .with_encoder_name(name)
        .with_bitrate(if codec == VtCodec::ProRes422Hq {
            0
        } else {
            2_000_000
        })
        .with_gop(6)
        .build(&mut output)
        .unwrap();
    let mut audio = sound.then(|| {
        AACEncoder::init(
            AudioInfo::new_raw(Sample::F32(SampleType::Packed), RATE, 1),
            128_000,
            &mut output,
        )
        .unwrap()
    });
    output.write_header().unwrap();
    let stream_tb = output.stream(0).unwrap().time_base();
    let encoder_tb = video.time_base();
    // Each packet a frame long, so the movie's last frame is shown too.
    let ticks = ((f64::from(encoder_tb.invert()) / f64::from(FPS)).round() as i64).max(1);
    let sink = |packet: &mut ffmpeg::Packet, output: &mut ffmpeg::format::context::Output| {
        if packet.duration() <= 0 {
            packet.set_duration(ticks);
        }
        packet.set_stream(0);
        packet.rescale_ts(encoder_tb, stream_tb);
        packet.write_interleaved(output)
    };
    for i in 0..FRAMES {
        let mut frame = ffmpeg::frame::Video::new(Pixel::NV12, 160, 96);
        frame.data_mut(0).fill(level(i));
        frame.data_mut(1).fill(128);
        let us = i as f64 * 1e6 / f64::from(FPS);
        let pts = (us * f64::from(encoder_tb.invert()) / 1e6).round() as i64;
        video
            .encode_to(&mut frame, pts, false, &mut |p| sink(p, &mut output))
            .unwrap();
    }
    video.flush_to(&mut |p| sink(p, &mut output)).unwrap();
    if let Some(aac) = audio.as_mut() {
        let total = RATE as usize;
        let mut at = 0;
        while at < total {
            let n = (total - at).min(1024);
            let block: Vec<f32> = (at..at + n)
                .map(|k| {
                    let loud = if k >= total / 2 { 0.5 } else { 0.0 };
                    loud * (k as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin()
                })
                .collect();
            let mut frame =
                ffmpeg::frame::Audio::new(Sample::F32(SampleType::Packed), n, ChannelLayout::MONO);
            frame.set_rate(RATE);
            frame.set_pts(Some(at as i64));
            frame.data_mut(0)[..n * 4].copy_from_slice(bytemuck::cast_slice(&block));
            aac.send_frame(frame, Duration::MAX, &mut output).unwrap();
            at += n;
        }
        aac.flush(&mut output).unwrap();
    }
    output.write_trailer().unwrap();
}

/// A file's picture as it decodes: each frame's mean luma (in 8-bit steps, read from the plane as stored, so a range
/// or a bit depth is never converted on the way) and its start (µs from the first frame), in the order shown, and the
/// stream's colour tags (range, matrix, primaries, transfer).
struct Decoded {
    levels: Vec<u8>,
    starts: Vec<i64>,
    tags: (
        ffmpeg::color::Range,
        ffmpeg::color::Space,
        ffmpeg::color::Primaries,
        ffmpeg::color::TransferCharacteristic,
    ),
}

fn decode(path: &Path) -> Decoded {
    let mut input = ffmpeg::format::input(path).unwrap();
    let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
    let (index, tb) = (stream.index(), stream.time_base());
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    let tags = (
        decoder.color_range(),
        decoder.color_space(),
        decoder.color_primaries(),
        decoder.color_transfer_characteristic(),
    );
    let (mut levels, mut starts) = (Vec::new(), Vec::new());
    let mut frame = ffmpeg::frame::Video::empty();
    let mut take = |decoder: &mut ffmpeg::decoder::Video| {
        while decoder.receive_frame(&mut frame).is_ok() {
            let deep = matches!(
                frame.format(),
                Pixel::YUV420P10LE | Pixel::YUV422P10LE | Pixel::P010LE
            );
            let (w, h, stride) = (
                frame.width() as usize,
                frame.height() as usize,
                frame.stride(0),
            );
            let plane = frame.data(0);
            let sum: u64 = (0..h)
                .map(|y| {
                    let row = &plane[y * stride..];
                    (0..w)
                        .map(|x| match deep {
                            true => {
                                u64::from(u16::from_le_bytes([row[2 * x], row[2 * x + 1]]) >> 2)
                            }
                            false => u64::from(row[x]),
                        })
                        .sum::<u64>()
                })
                .sum();
            levels.push((sum / (w * h) as u64) as u8);
            let pts = frame.timestamp().or_else(|| frame.pts()).unwrap();
            starts.push((pts as f64 * f64::from(tb) * 1e6).round() as i64);
        }
    };
    for (s, packet) in input.packets() {
        if s.index() == index {
            decoder.send_packet(&packet).unwrap();
            take(&mut decoder);
        }
    }
    decoder.send_eof().unwrap();
    take(&mut decoder);
    let first = starts.first().copied().unwrap_or(0);
    for s in &mut starts {
        *s -= first;
    }
    Decoded {
        levels,
        starts,
        tags,
    }
}

/// Each frame's mean luma, in the order the file shows them.
fn levels(path: &Path) -> Vec<u8> {
    decode(path).levels
}

fn spec(range: RangeUs) -> Reverse {
    Reverse {
        range,
        video_bps: 2_000_000,
        audio_bps: 128_000,
        threads: 2,
        // Six frames of 160 × 96 at a time: four windows over the second, each decoded from its own keyframe.
        buffer_bytes: 6 * 160 * 96 * 3 / 2,
    }
}

/// Close enough: a codec moves a flat grey level by a step or two.
fn near(got: &[u8], want: &[u8]) -> bool {
    got.len() == want.len() && got.iter().zip(want).all(|(a, b)| a.abs_diff(*b) <= 3)
}

#[test]
fn a_reversed_copy_shows_the_frames_last_first() {
    let dir = tempfile::tempdir().unwrap();
    let (src, out) = (
        dir.path().join("ramp.mov"),
        dir.path().join("ramp reversed.mov"),
    );
    ramp(&src, false);
    let mut heard = Vec::new();
    let range = RangeUs {
        start: 0,
        end: 1_000_000,
    };
    let info = write(
        &src,
        &out,
        &spec(range),
        &AtomicBool::new(false),
        &mut |p| heard.push(p),
    )
    .unwrap();
    // The source as it decodes, every frame brighter than the one before: backward.
    let source = levels(&src);
    assert_eq!(source.len(), FRAMES);
    assert!(source.windows(2).all(|w| w[0] < w[1]), "{source:?}");
    let want: Vec<u8> = source.into_iter().rev().collect();
    let got = levels(&out);
    assert!(near(&got, &want), "got {got:?}, want {want:?}");
    assert_eq!(
        (info.frames, info.width, info.height),
        (FRAMES as u64, 160, 96)
    );
    assert_eq!(info.duration_us, 1_000_000);
    assert!(!info.has_audio);
    // Progress rises to the end.
    assert!(heard.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(heard.last().copied(), Some(1.0));
    // Nothing is left beside it.
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[test]
fn only_the_range_is_reversed() {
    let dir = tempfile::tempdir().unwrap();
    let (src, out) = (dir.path().join("ramp.mov"), dir.path().join("part.mov"));
    ramp(&src, false);
    // Frames 6 to 17: from 0.25 s to 0.75 s.
    let range = RangeUs {
        start: 250_000,
        end: 750_000,
    };
    let info = write(
        &src,
        &out,
        &spec(range),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    let want: Vec<u8> = levels(&src)[6..18].iter().rev().copied().collect();
    let got = levels(&out);
    assert!(near(&got, &want), "got {got:?}, want {want:?}");
    assert_eq!(info.duration_us, 500_000);
    assert_eq!(info.held, range);
}

#[test]
fn its_sound_plays_backward_too() {
    let dir = tempfile::tempdir().unwrap();
    let (src, out) = (
        dir.path().join("ramp.mov"),
        dir.path().join("loud first.mov"),
    );
    ramp(&src, true);
    let range = RangeUs {
        start: 0,
        end: 1_000_000,
    };
    let info = write(
        &src,
        &out,
        &spec(range),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    assert!(info.has_audio);
    // Loud first, then silent: the source's silence-then-tone backward.
    let probe = decode::probe(&out).unwrap();
    let channels = usize::from(probe.channels);
    let mut energy = [0.0f64; 2];
    let mut emit = |at: u64, samples: &[f32]| -> Result<bool, String> {
        for (k, frame) in samples.chunks_exact(channels).enumerate() {
            let t = at as usize + k;
            // Leave out the edges, where the encoder's priming and the cut in the middle blur.
            let half = if (2_400..21_600).contains(&t) {
                0
            } else if (26_400..45_600).contains(&t) {
                1
            } else {
                continue;
            };
            energy[half] += f64::from(frame[0]).powi(2);
        }
        Ok(true)
    };
    decode::decode(&out, 0, None, &mut emit).unwrap();
    assert!(energy[0] > 100.0 * energy[1].max(1e-9), "{energy:?}");
}

#[test]
fn a_stopped_copy_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let (src, out) = (dir.path().join("ramp.mov"), dir.path().join("stopped.mov"));
    ramp(&src, false);
    let range = RangeUs {
        start: 0,
        end: 1_000_000,
    };
    let stop = AtomicBool::new(true);
    assert_eq!(
        write(&src, &out, &spec(range), &stop, &mut |_| {}).unwrap_err(),
        ReverseError::Stopped
    );
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
}

/// Two copies written at once under one name each write a partial of their own: neither reads the other's half-written
/// file, and nothing but the copy is left.
#[test]
fn two_copies_at_once_never_share_a_partial() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("ramp.mov");
    ramp(&src, false);
    let out = dir.path().join("same.mov");
    let range = RangeUs {
        start: 0,
        end: 1_000_000,
    };
    std::thread::scope(|s| {
        let runs: Vec<_> = (0..2)
            .map(|_| {
                s.spawn(|| {
                    write(
                        &src,
                        &out,
                        &spec(range),
                        &AtomicBool::new(false),
                        &mut |_| {},
                    )
                })
            })
            .collect();
        for run in runs {
            assert!(run.join().unwrap().is_ok());
        }
    });
    let want: Vec<u8> = levels(&src).into_iter().rev().collect();
    assert!(near(&levels(&out), &want));
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

/// Five frames of 160 × 96 at a time: windows start inside the fixtures' 12-frame GOPs.
fn small(range: RangeUs) -> Reverse {
    Reverse {
        buffer_bytes: 5 * 160 * 96 * 3 / 2,
        ..spec(range)
    }
}

fn range(start: i64, end: i64) -> RangeUs {
    RangeUs { start, end }
}

/// The source's frames a range shows: the one on screen at its start, then each that starts inside it.
fn shown(src: &Decoded, r: RangeUs) -> std::ops::Range<usize> {
    let first = src
        .starts
        .partition_point(|&s| s <= r.start)
        .saturating_sub(1);
    first..src.starts.partition_point(|&s| s < r.end)
}

/// Exactly the frames, backward: a duplicated or lost frame is a step of 4 off, more than a codec's rounding.
fn exactly(got: &[u8], want: &[u8]) -> bool {
    got.len() == want.len() && got.iter().zip(want).all(|(a, b)| a.abs_diff(*b) <= 2)
}

#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn a_b_frame_source_reverses_every_frame_at_every_window_edge() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = support::fixture("reverse-bframes.mp4");
    let src = decode(&src_path);
    // The whole file, then 7 frames from every frame on: some window starts on every frame of a GOP, its last included.
    let frame = |n: i64| n * 1_000_000 / 24;
    let ranges =
        std::iter::once(range(0, 2_000_000)).chain((0..41).map(|n| range(frame(n), frame(n + 7))));
    for (k, r) in ranges.enumerate() {
        let out = dir.path().join(format!("b{k}.mov"));
        write(
            &src_path,
            &out,
            &small(r),
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        let want: Vec<u8> = src.levels[shown(&src, r)].iter().rev().copied().collect();
        let got = levels(&out);
        assert!(exactly(&got, &want), "{r:?}: got {got:?}, want {want:?}");
    }
}

#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn a_turned_clip_reverses_upright() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = support::fixture("reverse-rotated.mov");
    let out = dir.path().join("turned.mov");
    let r = range(0, 2_000_000);
    write(
        &src_path,
        &out,
        &small(r),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    let probe = klipolo_engine::render::probe::probe_media(&out).unwrap();
    assert_eq!((probe.width, probe.height, probe.rotation), (96, 160, 0));
    let src = decode(&src_path);
    let want: Vec<u8> = src.levels[shown(&src, r)].iter().rev().copied().collect();
    assert!(exactly(&levels(&out), &want));
}

#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn a_variable_rate_clip_keeps_each_frame_s_length() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = support::fixture("reverse-vfr.mov");
    let src = decode(&src_path);
    let r = range(0, 3_000_000);
    let out = dir.path().join("vfr.mov");
    write(
        &src_path,
        &out,
        &small(r),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    let copy = decode(&out);
    let n = src.levels.len();
    assert!(exactly(
        &copy.levels,
        &src.levels.iter().rev().copied().collect::<Vec<_>>()
    ));
    // Output frame i is source frame n - 1 - i, as long as it was (the source's last frame has no next to say how long
    // it is: its length is the copy's choice).
    let length = |starts: &[i64], i: usize| starts[i + 1] - starts[i];
    for i in 1..n - 1 {
        let (got, want) = (length(&copy.starts, i), length(&src.starts, n - 1 - i));
        assert!(
            (got - want).abs() <= 1_000,
            "frame {i}: {got} µs, the source's {want} µs"
        );
    }
}

#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn a_full_range_bt601_clip_keeps_its_values_and_its_tags() {
    use ffmpeg::color::{Primaries, Range, Space, TransferCharacteristic};
    let dir = tempfile::tempdir().unwrap();
    let src_path = support::fixture("reverse-full-601.mp4");
    let src = decode(&src_path);
    let out = dir.path().join("full.mov");
    write(
        &src_path,
        &out,
        &small(range(0, 1_000_000)),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap();
    let copy = decode(&out);
    // H.264 states full range; ProRes is limited range by definition, so there the picture is squeezed to it.
    let full = test_codec().0 == VtCodec::H264High;
    let range_tag = if full { Range::JPEG } else { Range::MPEG };
    assert_eq!(
        copy.tags,
        (
            range_tag,
            Space::BT470BG,
            Primaries::BT470BG,
            TransferCharacteristic::SMPTE170M
        )
    );
    let squeeze = |v: u8| match full {
        true => v,
        false => (16 + (u32::from(v) * 219 + 127) / 255) as u8,
    };
    let want: Vec<u8> = src.levels.iter().rev().map(|&v| squeeze(v)).collect();
    assert!(
        exactly(&copy.levels, &want),
        "got {:?}, want {want:?}",
        copy.levels
    );
}

#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn an_hdr_clip_is_written_as_hdr_or_refused() {
    use ffmpeg::color::{Primaries, Space, TransferCharacteristic};
    let dir = tempfile::tempdir().unwrap();
    let src_path = support::fixture("reverse-hlg.mov");
    let out = dir.path().join("hlg.mov");
    let written = write(
        &src_path,
        &out,
        &small(range(0, 1_000_000)),
        &AtomicBool::new(false),
        &mut |_| {},
    );
    match encoder_name(VideoCodec::HevcMain10) {
        // Never flattened to 8-bit SDR.
        None => {
            assert_eq!(written.unwrap_err(), ReverseError::NoHdrEncoder);
            assert!(!out.exists());
        }
        Some(_) => {
            written.unwrap();
            let copy = decode(&out);
            assert_eq!(
                (copy.tags.1, copy.tags.2, copy.tags.3),
                (
                    Space::BT2020NCL,
                    Primaries::BT2020,
                    TransferCharacteristic::ARIB_STD_B67
                )
            );
            let src = decode(&src_path);
            let want: Vec<u8> = src.levels.iter().rev().copied().collect();
            assert!(exactly(&copy.levels, &want));
        }
    }
}

#[test]
#[ignore = "needs scripts/gen-engine-fixtures.sh"]
fn a_short_stretch_of_a_long_file_reads_only_near_it() {
    let dir = tempfile::tempdir().unwrap();
    let src_path = support::fixture("reverse-long-120s.mp4");
    let out = dir.path().join("short.mov");
    let mut heard = Vec::new();
    let r = range(100_000_000, 101_000_000);
    let spec = Reverse {
        buffer_bytes: 64 << 20,
        ..spec(r)
    };
    let info = write(&src_path, &out, &spec, &AtomicBool::new(false), &mut |p| {
        heard.push(p)
    })
    .unwrap();
    // The file has 2,880 frames: a second and a GOP or two around the range, never the whole file.
    assert!(
        info.scanned_packets < 300,
        "{} packets read",
        info.scanned_packets
    );
    // The pill moves from the start: it hears 0 at once, before the first pass reads anything.
    assert_eq!(heard.first(), Some(&0.0), "{heard:?}");
    let src = decode(&src_path);
    let want: Vec<u8> = src.levels[shown(&src, r)].iter().rev().copied().collect();
    assert!(exactly(&levels(&out), &want));
}
