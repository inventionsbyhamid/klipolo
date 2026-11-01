//! A stretch of a video played backward, as a file of its own (a reversed clip's footage): the picture of `range` of a
//! source written last frame first, and its sound backward too, so the preview and every export play it forward like
//! any file. Nothing decodes backward while it plays.
//!
//! - **Times:** the copy runs on the source's file clock turned around: its time `t` is the source's `range.end - t`.
//!   Source frame `j`, on screen over `[p_j, p_j+1)`, is on screen in the copy over `[end - p_j+1, end - p_j)`, so a
//!   variable frame rate keeps every frame's length. The frame on screen at `range.start` (it may start before it) is
//!   the copy's last.
//! - **Picture:** read twice. A pass over the packets alone (nothing decoded) finds the frames' times around the range,
//!   from the keyframe a second before it to a second past it (never the whole file); then the range
//!   is decoded window by window from its end, each window from the keyframe before it, at most `Reverse::buffer_bytes`
//!   of upright NV12 pictures held at once, and written newest first. The source's size and quarter turn are kept, the
//!   picture turned upright (`small_copy::turn_nv12`), so the copy needs no turn of its own. Encoded by the platform's
//!   H.264 encoder (VideoToolbox) at `Reverse::video_bps`; where there is none (Linux, for now) as ProRes 422 HQ.
//! - **Colour:** the source's tags (range, matrix, primaries, transfer) go on the copy and its values stay as they were
//!   (the scaler keeps the range). ProRes is limited range by definition, so there a full-range source is squeezed to
//!   it and tagged so. An HDR source (HLG or PQ) is written as 10-bit HEVC (P010), where the platform has that encoder,
//!   and refused where it doesn't (`NoHdrEncoder`): it is never cut to 8-bit SDR.
//! - **Sound:** the range's sound (the engine's decode, 48 kHz, its first two channels) held whole and written last
//!   sample first as AAC, placed by the source's `audio_lead_us` as the preview plays it. Past `MAX_SOUND_SECONDS`, or
//!   for a source without sound, the copy has none.
//! - **Writing:** a hidden temporary file of its own beside `out`, renamed into place once whole and removed on failure
//!   or stop. `cancel` is checked between packets and frames; `progress` hears 0 at once, then 0 to 1 through the first
//!   pass, the sound and the frames written (`SCAN_SHARE`, `SOUND_SHARE`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ffmpeg::format::Pixel;
use ffmpeg::format::context::Output;
use ffmpeg::format::sample::{Sample, Type as SampleType};
use ffmpeg::format::stream::Disposition;
use ffmpeg::software::scaling;
use ffmpeg::{ChannelLayout, Rational};
use klipolo_model::time::{RangeUs, Us};

use super::scan::tile::plain_layout;
use super::scan::{next_packet, set_discards};
use super::small_copy::{open_decoder, take_software, turn_nv12, turn_p010, write_packet};
use crate::audio::decode;
use crate::audio::mix::RATE;
use crate::export::codecs::{builder, encoder_name};
use crate::export::quality::VideoCodec;
use crate::platform::encode::{Backend, CodecRole, candidates};
use crate::render::probe::{MediaKind, probe_media, stream_start};
use crate::vendor::cap::enc_ffmpeg::aac::AACEncoder;
use crate::vendor::cap::enc_ffmpeg::h264::{ColorTags, H264Encoder};
use crate::vendor::cap::media_info::{AudioInfo, VideoInfo};

/// The longest range whose sound is reversed too: its 48 kHz stereo samples are held whole (about 230 MB for this).
pub const MAX_SOUND_SECONDS: i64 = 600;
/// Seconds between keyframes in the copy, so it scrubs well.
const GOP_SECONDS: f64 = 1.0;
/// Sound handed to the encoder at a time.
const BLOCK: usize = 4_096;
/// How far ahead of the range the passes start reading: a frame shown at its start may begin before it.
const SEEK_MARGIN_US: Us = 1_000_000;
/// How far past the range's end the first pass reads: B-frames shown in the range are decoded after frames past it.
const REORDER_US: Us = 1_000_000;
/// How far ahead of a window's first frame its seek asks for: more than any B-frame delay, so the keyframe it lands on
/// is shown before that frame even where a demuxer seeks by decode time (the frames before it are skipped).
const WINDOW_LEAD_US: Us = 250_000;
/// The shares of the progress the first pass and the sound take; the frames written take the rest.
const SCAN_SHARE: f64 = 0.05;
const SOUND_SHARE: f64 = 0.05;

/// What to reverse and how.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reverse {
    /// The part of the source, on its file clock.
    pub range: RangeUs,
    /// The picture's average bitrate (bits a second; ProRes ignores it).
    pub video_bps: u32,
    /// The sound's bitrate; 0 leaves the sound out.
    pub audio_bps: u32,
    /// The software decoder's frame threads at most; 0 lets FFmpeg choose.
    pub threads: usize,
    /// The decoded pictures held at once, at most (one window's).
    pub buffer_bytes: usize,
}

/// What a copy holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReversedInfo {
    /// Upright, as the source is shown.
    pub width: u32,
    pub height: u32,
    /// The source's average frame rate.
    pub fps: f64,
    pub frames: u64,
    /// The copy's length: the range's, from its first frame on.
    pub duration_us: Us,
    /// The stretch of the source it holds, `duration_us` long: the copy's time `t` is the source's `held.end - t`. Its end
    /// is the range's, or the file's last frame's end before it.
    pub held: RangeUs,
    pub has_audio: bool,
    pub bytes: u64,
    /// How many of the source's picture packets the first pass read: the stretch around the range, never the whole file.
    pub scanned_packets: u64,
}

/// Why no copy was made. The messages name no file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReverseError {
    #[error("the file has no video")]
    NoVideo,
    /// An HDR (HLG or PQ) source where this computer has no 10-bit HEVC encoder: never flattened to SDR.
    #[error("this computer can't write HDR video")]
    NoHdrEncoder,
    #[error("this computer has no video encoder for it")]
    NoEncoder,
    #[error("stopped")]
    Stopped,
    #[error("{0}")]
    Failed(String),
}

fn failed(what: &str, e: impl std::fmt::Display) -> ReverseError {
    ReverseError::Failed(format!("{what}: {e}"))
}

/// Writes `spec.range` of `src` backward to `out` (a QuickTime movie), as the module says. Blocking.
pub fn write(
    src: &Path,
    out: &Path,
    spec: &Reverse,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<ReversedInfo, ReverseError> {
    let _ = ffmpeg::init();
    if spec.range.end <= spec.range.start || spec.range.start < 0 {
        return Err(ReverseError::Failed("an empty range".into()));
    }
    let tmp = partial_path(out);
    let written = write_to(src, &tmp, spec, cancel, progress).and_then(|info| {
        std::fs::rename(&tmp, out).map_err(|e| failed("rename", e))?;
        Ok(info)
    });
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// `.<out>.<random>.partial.mov` beside `out`: hidden, this write's own (two writes to one name never share it), and
/// the same container, so the muxer is the same. A crash's leftovers end in `PARTIAL_SUFFIX` for a sweep to find.
fn partial_path(out: &Path) -> PathBuf {
    let name = out.file_name().unwrap_or_default().to_string_lossy();
    out.with_file_name(format!(
        ".{name}.{}{PARTIAL_SUFFIX}",
        uuid::Uuid::new_v4().simple()
    ))
}

/// How a partial copy's name ends (`partial_path`).
pub const PARTIAL_SUFFIX: &str = ".partial.mov";

/// The codec the copy is written in: H.264 where this computer has an encoder for it, else ProRes.
fn codec() -> Option<VideoCodec> {
    [VideoCodec::H264High, VideoCodec::ProRes422Hq]
        .into_iter()
        .find(|&c| encoder_name(c).is_some())
}

/// The source's picture frames over the range, by time on the file clock: `starts[j]` is frame `j`'s, ascending, the
/// first the frame on screen at the range's start; `end` is where the last stops showing (the range's end, or the
/// file's last frame's end before it).
struct Frames {
    /// The picture packets read to find them.
    scanned: u64,
    starts: Vec<Us>,
    end: Us,
}

impl Frames {
    /// Where frame `j` stops showing.
    fn end_of(&self, j: usize) -> Us {
        self.starts.get(j + 1).copied().unwrap_or(self.end)
    }
}

/// The video stream's index, time base and start, and its decoder's parameters.
struct Stream {
    index: usize,
    tb: Rational,
    start: i64,
    params: ffmpeg::codec::Parameters,
}

fn video_stream(input: &ffmpeg::format::context::Input) -> Result<Stream, ReverseError> {
    let stream = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .filter(|s| !s.disposition().contains(Disposition::ATTACHED_PIC))
        .ok_or(ReverseError::NoVideo)?;
    Ok(Stream {
        index: stream.index(),
        tb: stream.time_base(),
        start: stream_start(&stream),
        params: stream.parameters(),
    })
}

/// A timestamp of the stream as µs on the file clock.
fn us_of(stream: &Stream, ts: i64) -> Us {
    ((ts - stream.start) as f64 * f64::from(stream.tb) * 1e6).round() as Us
}

/// `t` on the file clock as a seek's timestamp (AV_TIME_BASE, µs, on the container's clock): a keyframe at or before it.
fn seek_ts(stream: &Stream, t: Us) -> i64 {
    t + (stream.start as f64 * f64::from(stream.tb) * 1e6).round() as i64
}

/// The first pass: the picture packets' times around the range, nothing decoded, kept for the range (`Frames`). It
/// starts at the keyframe before `SEEK_MARGIN_US` ahead of the range and stops once packets decode past its end by
/// `REORDER_US` (a frame shown in the range is decoded before that), so a short stretch of a long file reads only
/// around it. `progress` hears 0 to 1 as it goes.
fn frame_times(
    src: &Path,
    range: RangeUs,
    fps: f64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Frames, ReverseError> {
    let mut input = super::open::input(src).map_err(|e| failed("open", e))?;
    let stream = video_stream(&input)?;
    set_discards(&mut input, Some(stream.index), None);
    let from = range.start - SEEK_MARGIN_US;
    if from > 0 {
        let ts = seek_ts(&stream, from);
        if input.seek(ts, ..ts).is_err() {
            // It can't seek there: read from the start.
            input = super::open::input(src).map_err(|e| failed("open", e))?;
            set_discards(&mut input, Some(stream.index), None);
        }
    }
    let until = range.end + REORDER_US;
    let span = (until - from.max(0)).max(1) as f64;
    let (mut times, mut scanned) = (Vec::new(), 0u64);
    while let Some((index, packet)) = next_packet(&mut input).map_err(|e| failed("read", e))? {
        if cancel.load(Ordering::Relaxed) {
            return Err(ReverseError::Stopped);
        }
        if index != stream.index {
            continue;
        }
        scanned += 1;
        if let Some(pts) = packet.pts() {
            times.push(us_of(&stream, pts));
        }
        if let Some(decoded) = packet.dts().or(packet.pts()).map(|t| us_of(&stream, t)) {
            progress(((decoded - from.max(0)) as f64 / span).clamp(0.0, 1.0));
            if decoded > until {
                break;
            }
        }
    }
    times.sort_unstable();
    times.dedup();
    // The frame on screen at the range's start, then every one that starts inside it.
    let first = times
        .partition_point(|&t| t <= range.start)
        .saturating_sub(1);
    let starts: Vec<Us> = times[first..]
        .iter()
        .copied()
        .take_while(|&t| t < range.end)
        .collect();
    let Some(&last) = starts.last() else {
        return Err(ReverseError::Failed("no picture in this range".into()));
    };
    let frame_us = if fps > 0.0 {
        (1e6 / fps).round() as Us
    } else {
        33_333
    };
    // Past the file's last frame nothing shows: the copy ends there.
    let end = match times.get(first + starts.len()) {
        Some(_) => range.end,
        None => range.end.min(last + frame_us),
    };
    Ok(Frames {
        scanned,
        starts,
        end,
    })
}

/// Everything a run holds while it writes.
struct Writer {
    output: Output,
    video: H264Encoder,
    video_tb: Rational,
    encoder_tb: Rational,
    /// One frame at the source's average rate, in the encoder's time base: a packet's length when the encoder gives
    /// none (`put`).
    frame_ticks: i64,
    /// The reversed sound, interleaved (2 channels), and how many frames of it are written.
    sound: Option<(AACEncoder, Vec<f32>, usize)>,
    frames: u64,
}

/// One picture packet into the movie (`small_copy::write_packet`), `ticks` long when the encoder said nothing: the
/// QuickTime muxer's edit list ends where the last packet does, so a last packet of no length would never be shown.
fn put(
    packet: &mut ffmpeg::Packet,
    output: &mut Output,
    (from, to): (Rational, Rational),
    ticks: i64,
) -> Result<(), ffmpeg::Error> {
    if packet.duration() <= 0 {
        packet.set_duration(ticks);
    }
    write_packet(packet, output, from, to)
}

fn write_to(
    src: &Path,
    tmp: &Path,
    spec: &Reverse,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<ReversedInfo, ReverseError> {
    let probe = probe_media(src).map_err(|e| failed("probe", e))?;
    if probe.kind != MediaKind::Video {
        return Err(ReverseError::NoVideo);
    }
    let (tags, hdr) = source_color(src)?;
    // HDR stays HDR: a 10-bit HEVC copy with the source's tags, or none at all (never flattened to SDR).
    let (codec, format) = if hdr {
        encoder_name(VideoCodec::HevcMain10).ok_or(ReverseError::NoHdrEncoder)?;
        (VideoCodec::HevcMain10, Pixel::P010LE)
    } else {
        (codec().ok_or(ReverseError::NoEncoder)?, Pixel::NV12)
    };
    let even = |x: u32| (x / 2 * 2).max(2);
    let (width, height) = (even(probe.width), even(probe.height));
    let rotation = probe.rotation % 360;
    let stored = if rotation % 180 == 90 {
        (height, width)
    } else {
        (width, height)
    };
    let full = tags.range == ffmpeg::color::Range::JPEG;
    // ProRes is limited range by definition: a full-range source is squeezed to it there, and tagged so.
    let tags = match codec {
        VideoCodec::ProRes422Hq => ColorTags {
            range: ffmpeg::color::Range::MPEG,
            ..tags
        },
        _ => tags,
    };
    let shape = Shape {
        stored,
        rotation,
        format,
        full,
        squeeze: full && tags.range != ffmpeg::color::Range::JPEG,
    };
    let fps = if probe.fps > 0.0 { probe.fps } else { 30.0 };
    // The pill moves at once, then through the first pass and the sound, then frame by frame.
    progress(0.0);
    let frames = frame_times(src, spec.range, fps, cancel, &mut |done| {
        progress(SCAN_SHARE * done)
    })?;
    let (from, end) = (frames.starts[0].max(spec.range.start), frames.end);

    let mut output = ffmpeg::format::output_as(tmp, "mov").map_err(|e| failed("create", e))?;
    let info = VideoInfo {
        pixel_format: format,
        width,
        height,
        time_base: Rational(1, 1_000_000),
        frame_rate: Rational::from(fps),
    };
    let role = match codec {
        VideoCodec::HevcMain10 => CodecRole::Hevc10,
        _ => CodecRole::H264,
    };
    let hardware = codec != VideoCodec::ProRes422Hq
        && candidates(role)
            .first()
            .is_some_and(|c| c.backend == Backend::VideoToolbox);
    let build = |software: bool, output: &mut Output| {
        let mut b = builder(info, codec)
            .ok_or(ReverseError::NoEncoder)?
            .with_gop(((fps * GOP_SECONDS).round() as u32).max(1))
            .with_software_only(software)
            .with_color(tags);
        if codec != VideoCodec::ProRes422Hq {
            b = b.with_bitrate(spec.video_bps);
        }
        b.build(output).map_err(|e| failed("encoder", e))
    };
    let video = match build(false, &mut output) {
        Ok(encoder) => encoder,
        // A Mac without a media engine for this size: VideoToolbox's software encoder.
        Err(_) if hardware => build(true, &mut output)?,
        Err(e) => return Err(e),
    };
    let sound = reversed_sound(src, &probe, spec, (from, end), cancel, &mut |done| {
        progress(SCAN_SHARE + SOUND_SHARE * done)
    })?;
    let sound = match sound {
        Some(samples) => {
            let info = AudioInfo::new_raw(Sample::F32(SampleType::Packed), RATE, 2);
            let encoder = AACEncoder::init(info, spec.audio_bps as usize, &mut output)
                .map_err(|e| failed("sound encoder", e))?;
            Some((encoder, samples, 0))
        }
        None => None,
    };
    output.write_header().map_err(|e| failed("header", e))?;
    let video_tb = output
        .stream(0)
        .map(|s| s.time_base())
        .unwrap_or(Rational(1, 90_000));
    let encoder_tb = video.time_base();
    let mut writer = Writer {
        output,
        video,
        video_tb,
        encoder_tb,
        frame_ticks: ((f64::from(encoder_tb.invert()) / fps).round() as i64).max(1),
        sound,
        frames: 0,
    };
    writer.pictures(src, &frames, shape, spec, cancel, progress)?;
    writer.finish(end - from)?;
    let bytes = std::fs::metadata(tmp).map_err(|e| failed("size", e))?.len();
    Ok(ReversedInfo {
        width,
        height,
        fps,
        frames: writer.frames,
        duration_us: end - from,
        held: RangeUs { start: from, end },
        has_audio: writer.sound.is_some(),
        bytes,
        scanned_packets: frames.scanned,
    })
}

/// The sound of `[from, end)` on the file clock (48 kHz, two channels, interleaved), last frame first: what the copy
/// plays from its 0. None without sound, with `audio_bps` 0, or past `MAX_SOUND_SECONDS`.
fn reversed_sound(
    src: &Path,
    probe: &crate::render::probe::MediaInfo,
    spec: &Reverse,
    (from, end): (Us, Us),
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f64),
) -> Result<Option<Vec<f32>>, ReverseError> {
    if !probe.has_audio || spec.audio_bps == 0 || end - from > MAX_SOUND_SECONDS * 1_000_000 {
        return Ok(None);
    }
    let Ok(sound) = decode::probe(src) else {
        return Ok(None);
    };
    let channels = usize::from(sound.channels.max(1));
    let frame_of = |us: Us| (i128::from(us) * i128::from(RATE) / 1_000_000) as i64;
    // Sound time = file time − lead: where the range's sound is in the sound's own frames.
    let (first, until) = (
        frame_of(from - probe.audio_lead_us),
        frame_of(end - probe.audio_lead_us),
    );
    let total = (until - first).max(0) as usize;
    let mut out = vec![0.0f32; total * 2];
    if until > 0 {
        let start = first.max(0) as u64;
        let mut emit = |at: u64, samples: &[f32]| -> Result<bool, String> {
            if cancel.load(Ordering::Relaxed) {
                return Err("stopped".into());
            }
            for (k, frame) in samples.chunks_exact(channels).enumerate() {
                let i = at as i64 + k as i64 - first;
                if i < 0 {
                    continue;
                }
                if i as usize >= total {
                    progress(1.0);
                    return Ok(false);
                }
                let (l, r) = (frame[0], frame[channels.min(2) - 1]);
                out[i as usize * 2] = l;
                out[i as usize * 2 + 1] = r;
            }
            let read = (at as i64 + (samples.len() / channels) as i64 - first).max(0);
            progress((read as f64 / total.max(1) as f64).min(1.0));
            Ok(true)
        };
        let decoded = match decode::decode(src, start, Some(until as u64), &mut emit) {
            Err(decode::DecodeError::CantSeek) => {
                decode::decode(src, 0, Some(until as u64), &mut emit)
            }
            other => other,
        };
        if cancel.load(Ordering::Relaxed) {
            return Err(ReverseError::Stopped);
        }
        if let Err(e) = decoded {
            tracing::warn!(?e, "reversed copy: its sound couldn't be read; it has none");
            return Ok(None);
        }
    }
    // Last frame first: reverse the frames, keeping each frame's two channels in order.
    let mut frames: Vec<[f32; 2]> = out.chunks_exact(2).map(|f| [f[0], f[1]]).collect();
    frames.reverse();
    Ok(Some(frames.into_iter().flatten().collect()))
}

impl Writer {
    /// Decodes the range window by window from its end and writes each window's frames newest first.
    #[allow(clippy::too_many_arguments)]
    fn pictures(
        &mut self,
        src: &Path,
        frames: &Frames,
        shape: Shape,
        spec: &Reverse,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(f64),
    ) -> Result<(), ReverseError> {
        let mut input = super::open::input(src).map_err(|e| failed("open", e))?;
        let stream = video_stream(&input)?;
        set_discards(&mut input, Some(stream.index), None);
        let (mut decoder, _) = open_decoder(&stream.params, stream.tb, false, spec.threads)
            .map_err(|e| failed("decoder", e))?;
        let frame_bytes = shape.bytes();
        let per_window = (spec.buffer_bytes / frame_bytes).max(1);
        let count = frames.starts.len();
        let mut scaler: Option<(scaling::Context, (Pixel, u32, u32))> = None;
        let mut hi = count;
        while hi > 0 {
            let lo = hi.saturating_sub(per_window);
            let wanted = &frames.starts[lo..hi];
            let window = decode_window(
                &mut input,
                &mut decoder,
                &stream,
                wanted,
                shape,
                &mut scaler,
                cancel,
            )?;
            for (k, picture) in window.into_iter().enumerate().rev() {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ReverseError::Stopped);
                }
                let j = lo + k;
                let at = frames.end - frames.end_of(j);
                self.frame(picture, at)?;
                let done = self.frames as f64 / count as f64;
                // Exactly 1 at the last frame, whatever the sum rounds to.
                progress(match self.frames as usize == count {
                    true => 1.0,
                    false => SCAN_SHARE + SOUND_SHARE + (1.0 - SCAN_SHARE - SOUND_SHARE) * done,
                });
            }
            hi = lo;
        }
        Ok(())
    }

    /// Encodes `picture` at `at` (µs from the copy's 0), the sound first, up to its time.
    fn frame(&mut self, mut picture: ffmpeg::frame::Video, at: Us) -> Result<(), ReverseError> {
        self.sound_until(at)?;
        let pts = (at as f64 * f64::from(self.encoder_tb.invert()) / 1e6).round() as i64;
        let (output, video_tb, encoder_tb) = (&mut self.output, self.video_tb, self.encoder_tb);
        let ticks = self.frame_ticks;
        self.video
            .encode_to(&mut picture, pts, false, &mut |packet| {
                put(packet, output, (encoder_tb, video_tb), ticks)
            })
            .map_err(|e| failed("encode", e))?;
        self.frames += 1;
        Ok(())
    }

    /// Encodes the sound up to `at` (µs from the copy's 0).
    fn sound_until(&mut self, at: Us) -> Result<(), ReverseError> {
        let Some((encoder, samples, written)) = &mut self.sound else {
            return Ok(());
        };
        let want =
            ((at.max(0) as u64 * u64::from(RATE) / 1_000_000) as usize).min(samples.len() / 2);
        while *written < want {
            let n = (want - *written).min(BLOCK);
            let mut frame = ffmpeg::frame::Audio::new(
                Sample::F32(SampleType::Packed),
                n,
                ChannelLayout::STEREO,
            );
            frame.set_rate(RATE);
            frame.set_pts(Some(*written as i64));
            let block = &samples[*written * 2..(*written + n) * 2];
            frame.data_mut(0)[..n * 8].copy_from_slice(bytemuck::cast_slice(block));
            encoder
                .send_frame(frame, Duration::MAX, &mut self.output)
                .map_err(|e| failed("encode sound", e))?;
            *written += n;
        }
        Ok(())
    }

    /// The rest of the sound (`length` µs in all), both encoders' last packets and the movie's index.
    fn finish(&mut self, length: Us) -> Result<(), ReverseError> {
        self.sound_until(length)?;
        let (output, video_tb, encoder_tb) = (&mut self.output, self.video_tb, self.encoder_tb);
        let ticks = self.frame_ticks;
        self.video
            .flush_to(&mut |packet| put(packet, output, (encoder_tb, video_tb), ticks))
            .map_err(|e| failed("finish picture", e))?;
        if let Some((encoder, _, _)) = &mut self.sound {
            encoder
                .flush(&mut self.output)
                .map_err(|e| failed("finish sound", e))?;
        }
        self.output
            .write_trailer()
            .map_err(|e| failed("finish file", e))
    }
}

/// The frames starting at `wanted` (ascending, on the file clock), decoded from the keyframe before the first and made
/// upright NV12 of the shown size, in order. A frame the decoder never gives (a damaged stretch) takes the one before
/// it, or the next, so the copy keeps its length.
fn decode_window(
    input: &mut ffmpeg::format::context::Input,
    decoder: &mut ffmpeg::decoder::Video,
    stream: &Stream,
    wanted: &[Us],
    shape: Shape,
    scaler: &mut Option<(scaling::Context, (Pixel, u32, u32))>,
    cancel: &AtomicBool,
) -> Result<Vec<ffmpeg::frame::Video>, ReverseError> {
    let (first, last) = (wanted[0], wanted[wanted.len() - 1]);
    // In AV_TIME_BASE (µs), as seeks are: just ahead of the window's start on the container's clock; a keyframe at or
    // before it.
    let ts =
        first - WINDOW_LEAD_US + (stream.start as f64 * f64::from(stream.tb) * 1e6).round() as i64;
    if input.seek(ts, ..ts).is_err() {
        // It can't seek there: from the file's start.
        input.seek(0, ..0).map_err(|e| failed("seek", e))?;
    }
    decoder.flush();
    let mut got: Vec<Option<ffmpeg::frame::Video>> = (0..wanted.len()).map(|_| None).collect();
    let mut frame = ffmpeg::frame::Video::empty();
    let mut flushed = false;
    'read: loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ReverseError::Stopped);
        }
        if !flushed {
            match next_packet(input).map_err(|e| failed("read", e))? {
                Some((index, packet)) if index == stream.index => {
                    // A packet the decoder refuses (damaged) is passed over.
                    let _ = decoder.send_packet(&packet);
                }
                Some(_) => continue,
                None => {
                    let _ = decoder.send_eof();
                    flushed = true;
                }
            }
        }
        loop {
            match decoder.receive_frame(&mut frame) {
                Ok(()) => {}
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {
                    if flushed {
                        break 'read;
                    }
                    break;
                }
                Err(_) if flushed => break 'read,
                Err(ffmpeg::Error::Eof) => break 'read,
                Err(e) => return Err(failed("decode", e)),
            }
            let Some(pts) = frame.timestamp().or_else(|| frame.pts()) else {
                continue;
            };
            let at = us_of(stream, pts);
            if at > last {
                break 'read;
            }
            if let Ok(k) = wanted.binary_search(&at) {
                let mut software = take_software(&mut frame);
                got[k] = Some(upright(&mut software, shape, scaler)?);
            }
        }
    }
    // Damaged stretches: the frame before, else the next one decoded.
    let mut out = Vec::with_capacity(got.len());
    let mut before: Option<usize> = None;
    for k in 0..got.len() {
        if got[k].is_some() {
            before = Some(k);
        }
        let take = before.or_else(|| (k..got.len()).find(|&n| got[n].is_some()));
        let Some(n) = take else {
            return Err(ReverseError::Failed(
                "no picture could be decoded here".into(),
            ));
        };
        out.push(got[n].as_ref().expect("found above").clone());
    }
    Ok(out)
}

/// `frame` as NV12 of its stored size `(w, h)` (the shown size turned back), then turned `rotation` degrees upright.
fn upright(
    frame: &mut ffmpeg::frame::Video,
    shape: Shape,
    scaler: &mut Option<(scaling::Context, (Pixel, u32, u32))>,
) -> Result<ffmpeg::frame::Video, ReverseError> {
    // A JPEG-style frame (yuvj420p) is its plain layout at full range: read as such, swscale would squeeze it.
    plain_layout(frame);
    let (w, h) = shape.stored;
    let key = (frame.format(), frame.width(), frame.height());
    if scaler.as_ref().is_none_or(|(_, made)| *made != key) {
        let mut context = scaling::Context::get(
            key.0,
            key.1,
            key.2,
            shape.format,
            w,
            h,
            scaling::Flags::BILINEAR,
        )
        .map_err(|e| failed("scaler", e))?;
        keep_range(&mut context, shape.full);
        *scaler = Some((context, key));
    }
    let (context, _) = scaler.as_mut().expect("made above");
    let mut stored = ffmpeg::frame::Video::new(shape.format, w, h);
    context
        .run(frame, &mut stored)
        .map_err(|e| failed("scale", e))?;
    if shape.squeeze {
        squeeze_nv12(&mut stored);
    }
    if shape.rotation == 0 {
        return Ok(stored);
    }
    let (tw, th) = if shape.rotation % 180 == 90 {
        (h, w)
    } else {
        (w, h)
    };
    let mut turned = ffmpeg::frame::Video::new(shape.format, tw, th);
    match shape.format {
        Pixel::P010LE => turn_p010(&stored, &mut turned, shape.rotation),
        _ => turn_nv12(&stored, &mut turned, shape.rotation),
    }
    Ok(turned)
}

/// Squeezes an NV12 picture from full range to limited (luma 16 to 235, chroma 16 to 240) in place, for a copy whose
/// codec is limited range by definition (ProRes). Done by hand: swscale's plain copy from one 4:2:0 layout to another,
/// picked when the scaler is made, skips a range change asked for after.
fn squeeze_nv12(picture: &mut ffmpeg::frame::Video) {
    let luma: [u8; 256] = std::array::from_fn(|v| (16.0 + v as f32 * 219.0 / 255.0).round() as u8);
    let chroma: [u8; 256] =
        std::array::from_fn(|v| (128.0 + (v as f32 - 128.0) * 224.0 / 255.0).round() as u8);
    let (w, h) = (picture.width() as usize, picture.height() as usize);
    for (plane, table, rows) in [(0, &luma, h), (1, &chroma, h / 2)] {
        let stride = picture.stride(plane);
        let data = picture.data_mut(plane);
        for row in data.chunks_mut(stride).take(rows) {
            for v in &mut row[..w] {
                *v = table[usize::from(*v)];
            }
        }
    }
}

/// Makes `scaler` keep the source's range (`full` or limited, in and out): the copy carries the source's tags, so its
/// values must stay the source's.
fn keep_range(scaler: &mut scaling::Context, full: bool) {
    use ffmpeg::ffi::{sws_getColorspaceDetails, sws_setColorspaceDetails};
    let (mut inv_table, mut table) = (std::ptr::null_mut(), std::ptr::null_mut());
    let (mut src_range, mut dst_range) = (0, 0);
    let (mut brightness, mut contrast, mut saturation) = (0, 0, 0);
    // SAFETY: the context is valid for `scaler`'s life; FFmpeg fills the out-parameters with pointers into its own
    // tables, which `sws_setColorspaceDetails` takes back unchanged.
    unsafe {
        let ctx = scaler.as_mut_ptr();
        if sws_getColorspaceDetails(
            ctx,
            &mut inv_table,
            &mut src_range,
            &mut table,
            &mut dst_range,
            &mut brightness,
            &mut contrast,
            &mut saturation,
        ) >= 0
        {
            sws_setColorspaceDetails(
                ctx,
                inv_table,
                i32::from(full),
                table,
                i32::from(full),
                brightness,
                contrast,
                saturation,
            );
        }
    }
}

/// How the copy's pictures are made from the decoded frames: at the stored size, turned `rotation` degrees upright, in
/// `format` (NV12, or P010 for HDR), keeping the source's range (`full`), or squeezed to limited range (`squeeze`).
#[derive(Clone, Copy, Debug)]
struct Shape {
    stored: (u32, u32),
    rotation: u16,
    format: Pixel,
    full: bool,
    squeeze: bool,
}

impl Shape {
    /// One picture's bytes, held while its window is written.
    fn bytes(self) -> usize {
        let pixels = self.stored.0 as usize * self.stored.1 as usize;
        // 4:2:0: a luma sample a pixel and half as much chroma, of one byte (NV12) or two (P010).
        let sample = if self.format == Pixel::P010LE { 2 } else { 1 };
        (pixels * 3 / 2 * sample).max(1)
    }
}

/// The source's colour: its tags as its stream states them, whether it is full range, and whether it is HDR (HLG or
/// PQ, which only a 10-bit HEVC copy keeps).
fn source_color(src: &Path) -> Result<(ColorTags, bool), ReverseError> {
    use ffmpeg::color::{Range, TransferCharacteristic as Trc};
    let input = super::open::input(src).map_err(|e| failed("open", e))?;
    let stream = video_stream(&input)?;
    let decoder = ffmpeg::codec::Context::from_parameters(stream.params)
        .and_then(|c| c.decoder().video())
        .map_err(|e| failed("decoder", e))?;
    // A JPEG-style format is full range whatever it says.
    let jpeg = matches!(
        decoder.format(),
        Pixel::YUVJ420P | Pixel::YUVJ422P | Pixel::YUVJ444P
    );
    let range = if jpeg {
        Range::JPEG
    } else {
        decoder.color_range()
    };
    let tags = ColorTags {
        range,
        space: decoder.color_space(),
        primaries: decoder.color_primaries(),
        transfer: decoder.color_transfer_characteristic(),
    };
    let hdr = matches!(tags.transfer, Trc::SMPTE2084 | Trc::ARIB_STD_B67);
    Ok((tags, hdr))
}
