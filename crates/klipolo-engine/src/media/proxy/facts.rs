//! What a source file is, as far as editing copies care: its shown size, rate, codec, bit depth,
//! HDR, length, how far apart its keyframes are and whether this computer decodes it in hardware. Read through the
//! bundled libavformat in-process; nothing is decoded.

use std::path::Path;
use std::time::{Duration, Instant};

use klipolo_model::time::Us;

use crate::export::shared::FrameRate;
use crate::media::key::{SourceId, source_id};
use crate::media::scan::{next_packet, set_discards};
use crate::render::probe::{stream_rotation, video_stream};

/// Keyframes are measured over this much of the file's start.
const GOP_WINDOW_US: Us = 30_000_000;
/// A probe that takes longer than this (a damaged or endless file, a stalled share) is given up: try again later.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// A file that doesn't state its length is measured from this much of its end (a live-written WebM or Matroska holds
/// many clusters in it), then, if that finds no packet, from the whole file.
const TAIL_BYTES: u64 = 16 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct SourceFacts {
    /// The size as shown: anamorphic pixels widened, then a phone's quarter turn applied.
    pub width: u32,
    pub height: u32,
    /// The average frame rate as the file states it (its fraction: 24000/1001; 0/1 when it doesn't say). Imports store
    /// the same one (`probe_media`: the average, else the nominal rate), so the preview numbers the source's frames on
    /// it, and an editing copy is made at exactly this rate.
    pub rate: FrameRate,
    /// FFmpeg's codec name (`h264`, `hevc`, `prores`…).
    pub codec: String,
    /// More than 8 bits per sample.
    pub ten_bit: bool,
    /// Those deeper samples come through the hardware path (`platform::decode::hw_reads_deep`: 10-bit 4:2:0 HEVC on a
    /// Mac with HEVC hardware), so its depth alone doesn't make it scrub slowly. False for 8-bit files.
    pub ten_bit_hw: bool,
    /// PQ (SMPTE ST 2084) or HLG (ARIB STD-B67).
    pub hdr: bool,
    /// The largest gap between keyframes in the first 30 s (a single keyframe there counts as the whole window, or the
    /// whole file when it is shorter).
    pub max_key_interval_us: Us,
    /// The average and nominal rates differ by more than 1 % (a phone's variable rate).
    pub variable_rate: bool,
    /// This computer decodes the codec in hardware (`platform::decode::hw_decodes`).
    pub hw_decodes: bool,
    /// The video stream's length: as the file states it, else measured from its packets; 0 when neither works (no copy
    /// is made of such a file: `job::Refused::UnknownLength`).
    pub duration_us: Us,
    /// Where the video stream starts in the file (FFmpeg's start; 0 when it doesn't say): every decoder's frame 0 is the
    /// frame there, so an older copy of a source starting late is made again (`CopyRecord::current`).
    pub start_us: Us,
    /// The file these facts describe (its size, time and number when it was read, unchanged through the read). A job
    /// is only planned from facts of the file as it is (`ProxyJob::create`); None: not tied to a file (tests).
    pub id: Option<SourceId>,
}

/// Why a source's facts couldn't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeError {
    /// Reading took longer than its time limit (a stalled network share or drive): try again later.
    TimedOut,
    /// The file can't be opened or has no readable video (the sentence says which).
    Unreadable(String),
    /// The file changed while it was read (it is being written or replaced): try again later.
    Changing,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut => f.write_str("This file took too long to read."),
            Self::Changing => f.write_str("This file changed while it was read."),
            Self::Unreadable(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ProbeError {}

impl SourceFacts {
    /// The frame rate (0 when the file doesn't say).
    pub fn fps(&self) -> f64 {
        if self.rate.num == 0 {
            0.0
        } else {
            self.rate.f64()
        }
    }
}

fn rate(r: ffmpeg::Rational) -> Option<f64> {
    (r.numerator() > 0 && r.denominator() > 0)
        .then(|| f64::from(r.numerator()) / f64::from(r.denominator()))
}

fn to_us(v: i64, tb: ffmpeg::Rational) -> Us {
    let num = i128::from(v) * i128::from(tb.numerator()) * 1_000_000;
    (num / i128::from(tb.denominator().max(1))) as Us
}

/// Reads `path`'s facts, giving up after `PROBE_TIMEOUT`.
pub fn probe_facts(path: &Path) -> Result<SourceFacts, ProbeError> {
    probe_facts_within(path, PROBE_TIMEOUT)
}

/// `probe_facts` with its own time limit (tests use none at all).
pub fn probe_facts_within(path: &Path, timeout: Duration) -> Result<SourceFacts, ProbeError> {
    let before = path.to_str().and_then(source_id);
    let mut facts = read_facts(path, timeout)?;
    // The file read is the file as it was looked at, start to end; else the facts may mix two versions.
    if path.to_str().and_then(source_id) != before {
        return Err(ProbeError::Changing);
    }
    facts.id = before;
    Ok(facts)
}

fn read_facts(path: &Path, timeout: Duration) -> Result<SourceFacts, ProbeError> {
    let _ = ffmpeg::init();
    let deadline = Instant::now() + timeout;
    let late = move || Instant::now() > deadline;
    let unreadable = |message: String| {
        if late() {
            ProbeError::TimedOut
        } else {
            ProbeError::Unreadable(message)
        }
    };
    let mut input = crate::media::open::input_interruptible(path, late, ffmpeg::Dictionary::new())
        .map_err(|e| unreadable(format!("This file couldn't be read: {e}")))?;
    // SAFETY: the context is `input`'s own; a plain field.
    let guessed = unsafe {
        (*input.as_ptr()).duration_estimation_method
            == ffmpeg::ffi::AVDurationEstimationMethod::AVFMT_DURATION_FROM_BITRATE
    };
    let (index, tb, start_us, id, avg, nominal, stated, shown, decoder) = {
        let stream = video_stream(&input)
            .ok_or_else(|| ProbeError::Unreadable("This file has no video.".into()))?;
        // SAFETY: the parameters and stream belong to `input`, which outlives these reads; FFmpeg only reads them.
        let (w, h, sar) = unsafe {
            let par = stream.parameters().as_ptr();
            let sar = ffmpeg::ffi::av_guess_sample_aspect_ratio(
                input.as_ptr().cast_mut(),
                stream.as_ptr().cast_mut(),
                std::ptr::null_mut(),
            );
            ((*par).width, (*par).height, sar)
        };
        let (mut width, mut height) =
            (u32::try_from(w).unwrap_or(0), u32::try_from(h).unwrap_or(0));
        // Anamorphic sources widen before the display rotation, as the decoders' frames do.
        if sar.num > 0 && sar.den > 0 {
            let widened = f64::from(width) * f64::from(sar.num) / f64::from(sar.den);
            if widened.is_finite() && widened <= 16384.0 {
                width = widened.round() as u32;
            }
        }
        if stream_rotation(&stream) % 180 == 90 {
            std::mem::swap(&mut width, &mut height);
        }
        let tb = stream.time_base();
        // A length only guessed from the bitrate isn't stated: it is measured below.
        let stated = if stream.duration() > 0 {
            to_us(stream.duration(), tb)
        } else if guessed {
            0
        } else {
            input.duration().max(0)
        };
        let start_us = match stream.start_time() {
            i64::MIN => 0,
            t => to_us(t, tb),
        };
        let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .and_then(|c| c.decoder().video())
            .map_err(|e| unreadable(format!("This file's video couldn't be read: {e}")))?;
        (
            stream.index(),
            tb,
            start_us,
            stream.parameters().id(),
            stream.avg_frame_rate(),
            stream.rate(),
            stated,
            (width, height),
            decoder,
        )
    };
    if shown.0 == 0 || shown.1 == 0 {
        return Err(ProbeError::Unreadable(
            "This file's picture size couldn't be read.".into(),
        ));
    }
    // SAFETY: a static FFmpeg table entry; `depth` is a plain field.
    let depth = decoder
        .format()
        .descriptor()
        .map(|d| unsafe { (*d.as_ptr()).comp[0].depth })
        .filter(|&d| d > 0)
        .unwrap_or_else(|| {
            // SAFETY: the decoder context is valid; a plain field.
            unsafe { (*decoder.as_ptr()).bits_per_raw_sample }
        });
    use ffmpeg::color::TransferCharacteristic as Trc;
    let hdr = matches!(
        decoder.color_transfer_characteristic(),
        Trc::SMPTE2084 | Trc::ARIB_STD_B67
    );
    let variable_rate = match (rate(avg), rate(nominal)) {
        (Some(a), Some(r)) => (a - r).abs() > 0.01 * r,
        _ => false,
    };
    let exact = [avg, nominal]
        .into_iter()
        .find(|r| rate(*r).is_some())
        .map_or(FrameRate { num: 0, den: 1 }, |r| FrameRate {
            num: r.numerator() as u32,
            den: r.denominator() as u32,
        });
    let fps = rate(avg).or(rate(nominal)).unwrap_or(0.0);
    // No stated length: its packets say. The tail first, while the demuxer is fresh (one that has reached the end
    // doesn't read again after a byte seek), then back to the start for the keyframes; the whole file only when the
    // tail held no packet with a time.
    let mut duration_us = stated;
    if stated <= 0 {
        let size = std::fs::metadata(path).map_or(0, |m| m.len());
        duration_us = last_packet_end(
            &mut input,
            index,
            tb,
            fps,
            Some(size.saturating_sub(TAIL_BYTES)),
        )
        .map_or(0, |end| (end - start_us).max(0));
        rewind(&mut input, start_us);
    }
    let keys = key_spacing(&mut input, index, tb);
    if duration_us <= 0 && stated <= 0 {
        rewind(&mut input, start_us);
        duration_us = last_packet_end(&mut input, index, tb, fps, None)
            .map_or(0, |end| (end - start_us).max(0));
    }
    if late() {
        return Err(ProbeError::TimedOut);
    }
    Ok(SourceFacts {
        width: shown.0,
        height: shown.1,
        rate: exact,
        codec: id.name().to_owned(),
        ten_bit: depth > 8,
        ten_bit_hw: depth > 8 && crate::platform::decode::hw_reads_deep(id, decoder.format()),
        hdr,
        max_key_interval_us: keys.widest(duration_us),
        variable_rate,
        hw_decodes: crate::platform::decode::hw_decodes(id),
        duration_us,
        start_us,
        id: None,
    })
}

/// The keyframes seen over the first `GOP_WINDOW_US` (decode times, µs).
struct KeySpacing {
    first: Option<Us>,
    last_key: Option<Us>,
    widest: Us,
    /// Where the window ended, when a keyframe past it was seen.
    window_end: Option<Us>,
}

impl KeySpacing {
    /// The largest gap; the one after the last keyframe runs to the window's end (or the stream's, when shorter), so a
    /// single keyframe counts as the whole window.
    fn widest(&self, duration_us: Us) -> Us {
        let start = self.first.unwrap_or(0);
        let end = self
            .window_end
            .unwrap_or_else(|| (start + GOP_WINDOW_US).min(start + duration_us.max(0)));
        match self.last_key {
            Some(key) => self.widest.max(end - key),
            None => GOP_WINDOW_US,
        }
    }
}

/// The video stream's keyframes over its first `GOP_WINDOW_US`: only keyframes are read (`AVDISCARD_NONKEY`: an MP4's
/// demuxer skips the other samples by its index, without reading them) and every other stream is discarded.
fn key_spacing(
    input: &mut ffmpeg::format::context::Input,
    index: usize,
    tb: ffmpeg::Rational,
) -> KeySpacing {
    set_discards(input, None, Some(index));
    let mut keys = KeySpacing {
        first: None,
        last_key: None,
        widest: 0,
        window_end: None,
    };
    while let Ok(Some((stream, packet))) = next_packet(input) {
        if stream != index || !packet.is_key() {
            continue;
        }
        // Decode times: they come straight from the index, while a keyframes-only read can leave presentation times
        // a frame off when the stream has B-frames (the demuxer's composition offsets).
        let Some(t) = packet.dts().or(packet.pts()).map(|v| to_us(v, tb)) else {
            continue;
        };
        let start = *keys.first.get_or_insert(t);
        let end = start + GOP_WINDOW_US;
        if t > end {
            keys.window_end = Some(end);
            break;
        }
        if let Some(previous) = keys.last_key {
            keys.widest = keys.widest.max(t - previous);
        }
        keys.last_key = Some(t);
    }
    keys
}

/// Back to the stream's start, by time (which also resets a demuxer that reached the end).
fn rewind(input: &mut ffmpeg::format::context::Input, start_us: Us) {
    // SAFETY: `input`'s own context; a time in AV_TIME_BASE units (stream -1), anything at or before it.
    let sought = unsafe {
        ffmpeg::ffi::avformat_seek_file(input.as_mut_ptr(), -1, i64::MIN, start_us, start_us, 0)
    };
    if sought < 0 {
        tracing::debug!("editing copy probe: couldn't seek back to the start");
    }
}

/// The end (µs) of the video stream's last packet with a time, reading from byte `from` (a byte seek: Matroska's
/// demuxer resyncs at the next cluster) or from where the demuxer is (None) to the end; None when no packet has a time.
fn last_packet_end(
    input: &mut ffmpeg::format::context::Input,
    index: usize,
    tb: ffmpeg::Rational,
    fps: f64,
    from: Option<u64>,
) -> Option<Us> {
    set_discards(input, Some(index), None);
    if let Some(at) = from {
        // SAFETY: `input`'s own context; a byte position inside the file. A demuxer that can't seek by bytes refuses.
        let sought = unsafe {
            ffmpeg::ffi::av_seek_frame(
                input.as_mut_ptr(),
                -1,
                at as i64,
                ffmpeg::ffi::AVSEEK_FLAG_BYTE,
            )
        };
        if sought < 0 {
            return None;
        }
    }
    let frame_us = if fps > 0.0 { (1e6 / fps) as Us } else { 0 };
    let mut end = None::<Us>;
    while let Ok(Some((stream, packet))) = next_packet(input) {
        if stream != index {
            continue;
        }
        let Some(t) = packet.pts().or(packet.dts()) else {
            continue;
        };
        let packet_end = if packet.duration() > 0 {
            to_us(t + packet.duration(), tb)
        } else {
            to_us(t, tb) + frame_us
        };
        end = Some(end.map_or(packet_end, |e: Us| e.max(packet_end)));
    }
    end
}
