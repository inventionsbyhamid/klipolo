//! In-process probing through the bundled libavformat (never an ffprobe process): what an import stores about a file,
//! and what the decoders don't report (a stream's frame rate when the import didn't store one, and the display
//! rotation phones store portrait video with), plus what Sync reads (`sound`).

mod sound;

use std::path::Path;
use std::time::{Duration, Instant};

use ffmpeg::format::stream::Disposition;
use ffmpeg::media::Type;

pub use sound::AudioFormat;

/// What a file holds, as an import stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Audio,
    Image,
}

/// An imported file's facts.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub kind: MediaKind,
    /// Seconds; a still image counts as 5 s.
    pub duration: f64,
    /// The size as shown: anamorphic pixels widened, then a portrait phone's quarter turn applied. 0 × 0 for audio.
    pub width: u32,
    pub height: u32,
    /// Average frame rate of the video; 0 for audio, images and files that don't say.
    pub fps: f64,
    /// Clockwise degrees (0, 90, 180 or 270) that turn the video's stored frames upright (`probe_rotation`); 0 for
    /// audio and images.
    pub rotation: u16,
    pub has_audio: bool,
    /// The sound's first timestamp minus the picture's (µs), from each stream's start time: where the sound cache's
    /// first frame plays against the picture. 0 without both, and 0 (logged) past ±500 ms.
    pub audio_lead_us: i64,
    /// The container's `creation_time`, when stated (ms since 1970).
    pub created_ms: Option<i64>,
    /// The best audio stream's codec name, rate and channels (`DeviceSig`).
    pub audio: Option<AudioFormat>,
}

const STILL_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "bmp", "tif", "tiff", "webp", "heic", "avif",
];
/// A probe that takes longer than this (a damaged or endless file) is given up.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

fn rate(r: ffmpeg::Rational) -> Option<f64> {
    (r.numerator() > 0 && r.denominator() > 0)
        .then(|| f64::from(r.numerator()) / f64::from(r.denominator()))
}

fn seconds(stream: &ffmpeg::format::stream::Stream) -> Option<f64> {
    let tb = stream.time_base();
    (stream.duration() > 0 && tb.denominator() > 0)
        .then(|| stream.duration() as f64 * f64::from(tb.numerator()) / f64::from(tb.denominator()))
        .filter(|s| s.is_finite())
}

/// Reads a local file's kind, length, shown size, frame rate and audio presence. Errors are plain sentences.
pub fn probe_media(path: &Path) -> Result<MediaInfo, String> {
    let _ = ffmpeg::init();
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let ctx = crate::media::open::input_interruptible(
        path,
        move || Instant::now() > deadline,
        ffmpeg::Dictionary::new(),
    )
    .map_err(|e| format!("Could not read media metadata: {e}"))?;
    let video = video_stream(&ctx);
    let has_audio = ctx
        .streams()
        .any(|s| s.parameters().medium() == Type::Audio);
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let still = video.is_some()
        && (STILL_EXTENSIONS.contains(&extension.as_str())
            || ctx.format().name().ends_with("_pipe"));
    let kind = match (&video, still, has_audio) {
        (Some(_), true, _) => MediaKind::Image,
        (Some(_), false, _) => MediaKind::Video,
        (None, _, true) => MediaKind::Audio,
        (None, _, false) => {
            return Err("The file contains no supported image, video, or audio stream".into());
        }
    };
    let mut duration = if ctx.duration() > 0 {
        ctx.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
    } else {
        ctx.streams()
            .filter_map(|s| seconds(&s))
            .fold(0.0, f64::max)
    };
    if kind == MediaKind::Image {
        duration = 5.0;
    }
    if !duration.is_finite() || !(0.02..=86400.0).contains(&duration) {
        return Err("Media duration could not be read, or exceeds 24 hours".into());
    }
    let (mut width, mut height, mut fps, mut rotation) = (0, 0, 0.0, 0);
    if let Some(stream) = &video {
        // SAFETY: the parameters and stream belong to `ctx`, which outlives these reads; FFmpeg only reads them.
        let (w, h, sar) = unsafe {
            let par = stream.parameters().as_ptr();
            let sar = ffmpeg::ffi::av_guess_sample_aspect_ratio(
                ctx.as_ptr().cast_mut(),
                stream.as_ptr().cast_mut(),
                std::ptr::null_mut(),
            );
            ((*par).width, (*par).height, sar)
        };
        width = u32::try_from(w).unwrap_or(0);
        height = u32::try_from(h).unwrap_or(0);
        // Anamorphic sources widen before the display rotation, as the decoders' frames do.
        if sar.num > 0 && sar.den > 0 {
            let shown = f64::from(width) * f64::from(sar.num) / f64::from(sar.den);
            if !shown.is_finite() || shown > 16384.0 {
                return Err("Unsupported display aspect ratio".into());
            }
            width = shown.round() as u32;
        }
        let turn = stream_rotation(stream);
        if turn % 180 == 90 {
            std::mem::swap(&mut width, &mut height);
        }
        if kind == MediaKind::Video {
            fps = rate(stream.avg_frame_rate())
                .or_else(|| rate(stream.rate()))
                .unwrap_or(0.0);
            rotation = turn;
        }
    }
    if kind != MediaKind::Audio && (width == 0 || height == 0 || width > 16384 || height > 16384) {
        return Err("Unsupported image or video dimensions".into());
    }
    // The stream the sound cache decodes (`audio::decode`).
    let sound = ctx.streams().best(Type::Audio);
    let audio_lead_us = match (&video, &sound) {
        (Some(picture), Some(sound)) if kind == MediaKind::Video => {
            sound::lead_us(picture, sound, path)
        }
        _ => 0,
    };
    Ok(MediaInfo {
        kind,
        duration,
        width,
        height,
        fps,
        rotation,
        has_audio,
        audio_lead_us,
        created_ms: sound::created_ms(&ctx),
        audio: sound.as_ref().map(sound::format),
    })
}

/// Frame rate of the best video stream, exact (30000/1001 for 29.97); imports store 0 until probed (30 when the file
/// doesn't say).
pub fn probe_fps(path: &Path) -> f64 {
    let _ = ffmpeg::init();
    crate::media::open::input(path)
        .ok()
        .and_then(|ctx| {
            ctx.streams()
                .best(ffmpeg::media::Type::Video)
                .map(|s| s.avg_frame_rate())
        })
        .filter(|r| r.numerator() > 0 && r.denominator() > 0)
        .map(|r| f64::from(r.numerator()) / f64::from(r.denominator()))
        .unwrap_or(30.0)
}

/// The video stream an import describes: the first that isn't cover art (the track AVAssetReader reads). The
/// rotation fallback reads the same one, so a stored turn and a probed one never disagree.
pub(crate) fn video_stream(
    ctx: &ffmpeg::format::context::Input,
) -> Option<ffmpeg::format::stream::Stream<'_>> {
    ctx.streams().find(|s| {
        s.parameters().medium() == Type::Video
            && !s.disposition().contains(Disposition::ATTACHED_PIC)
    })
}

/// Where `stream` starts, in its time base: its first pts, or 0 when the file doesn't say. Every decoder numbers the
/// stream's frames from here (frame 0 is its first), and the stored keyframe index records it.
pub(crate) fn stream_start(stream: &ffmpeg::format::stream::Stream) -> i64 {
    match stream.start_time() {
        i64::MIN => 0,
        start => start,
    }
}

/// Clockwise degrees (0, 90, 180 or 270) that turn the import's video stream's frames upright. Phones store portrait
/// video as landscape frames plus a display matrix; the decoders return frames as stored, so the compositor turns
/// them (imports already store the upright size, see `probe_media`).
pub fn probe_rotation(path: &Path) -> u16 {
    let _ = ffmpeg::init();
    let Ok(ctx) = crate::media::open::input(path) else {
        return 0;
    };
    video_stream(&ctx).map_or(0, |stream| stream_rotation(&stream))
}

/// A stream's display matrix as clockwise quarter turns (0, 90, 180 or 270). The thumbnail pass reads it off the
/// stream it already has open, so it never probes the file again.
pub(crate) fn stream_rotation(stream: &ffmpeg::format::stream::Stream) -> u16 {
    stream
        .side_data()
        .find(|d| d.kind() == ffmpeg::codec::packet::side_data::Type::DisplayMatrix)
        .and_then(|d| {
            let bytes = d.data();
            let matrix: Vec<i32> = bytes
                .chunks_exact(4)
                .take(9)
                .map(|b| i32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
                .collect();
            // SAFETY: the display matrix is nine i32s (FFmpeg's AV_PKT_DATA_DISPLAYMATRIX layout).
            (matrix.len() == 9)
                .then(|| unsafe { ffmpeg::ffi::av_display_rotation_get(matrix.as_ptr()) })
        })
        // FFmpeg reports counter-clockwise degrees; quarter turns only.
        .map(|ccw| (((-ccw / 90.0).round() as i64).rem_euclid(4) * 90) as u16)
        .unwrap_or(0)
}
