//! The scene log's small copy of a clip (its proxy): MP4s of H.264 at 360p, 4 frames a second and a low bitrate with
//! AAC mono sound, each at most `CHUNK_US` long and overlapping the next by `OVERLAP_US`, cut at encode time so each
//! upload stays small enough for a slow link: about 9 MB for ten minutes, which goes up within the relay's 60 s on
//! about 1.2 Mb/s. The engine writes them (`klipolo_engine::media::small_copy`):
//! VideoToolbox on a Mac; Linux has no H.264 encoder, so tests there use a ready file (`existing`).
//!
//! **Cuts** are found while the copy is made, from every frame of the source at its own rate (the engine's
//! `EveryFrame`, into a `cuts::Finder` per chunk), so they land on the source's frames whatever frame rate the copy
//! keeps. A ready copy's are found by decoding it at its own rate (`existing`).
//!
//! **Times:** a chunk's time 0 is its `range.start` on the clip's file clock (its picture's), so every time measured in
//! a chunk (Spark's moments) plus `range.start` is the clip's own time. The cuts are in the clip's time already. The
//! sound is placed by the clip's audio lead, as the preview plays it (`ProxyInfo::audio_lead_us`).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use klipolo_engine::media::small_copy::{self, SmallCopy, SmallCopyError};
use klipolo_engine::render::probe::{MediaInfo, MediaKind, probe_media};
use klipolo_model::time::{RangeUs, Us, secs_to_us};

use crate::cuts::{self, Finder};
use crate::error::VideoError;

/// The longest chunk: what one Spark call watches.
pub const CHUNK_US: Us = 600_000_000;
/// How much each chunk overlaps the next: Spark is unreliable in a chunk's last seconds, and the next chunk sees them
/// with some lead-in.
pub const OVERLAP_US: Us = 20_000_000;

/// The box a copy's picture fits in, (long side, short side): 640 × 360 landscape, 360 × 640 portrait, the size Spark's
/// token count was measured at (131 tokens a frame).
pub const FIT: (u32, u32) = (640, 360);
/// Frames a second at most: Spark samples at most 2, so 4 keep every frame it looks at (the cuts come from the
/// source's own frames, not these).
pub const MAX_FPS: f64 = 4.0;
/// The picture's average bitrate: ten minutes is about 6 MB (clip A's measured 6.0 MB), a clean 360p picture at 4
/// frames a second.
pub const VIDEO_BPS: u32 = 80_000;
/// The sound's bitrate (AAC mono): Spark listens to it.
pub const AUDIO_BPS: u32 = 48_000;

/// One file of the copy.
#[derive(Clone, Debug, PartialEq)]
pub struct ProxyChunk {
    pub path: PathBuf,
    /// The part of the clip it holds, on the clip's file clock.
    pub range: RangeUs,
    pub bytes: u64,
}

/// A clip's small copy, in chunks.
#[derive(Clone, Debug, PartialEq)]
pub struct ProxyInfo {
    /// The clip's length (its picture's).
    pub duration_us: Us,
    /// Where the clip's sound starts against its picture (`MediaInfo::audio_lead_us`): a word heard at sound time t is
    /// at file time t + this.
    pub audio_lead_us: Us,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub has_audio: bool,
    /// In order, each overlapping the next.
    pub chunks: Vec<ProxyChunk>,
    /// The clip's hard cuts (file time, each the first frame of a new shot), found in every frame of the source. A
    /// lower bound (see `SceneLog::cuts`).
    pub cuts: Vec<Us>,
}

/// The chunks for a clip of `duration_us`: as few as keep each within `max_us`, all the same length, each overlapping
/// the next by `overlap_us`; the last ends at the clip's end. One chunk when the clip fits in one.
pub fn plan(duration_us: Us, max_us: Us, overlap_us: Us) -> Vec<RangeUs> {
    let duration = duration_us.max(0);
    if duration <= max_us || max_us <= overlap_us {
        return vec![RangeUs {
            start: 0,
            end: duration,
        }];
    }
    let step = max_us - overlap_us;
    let n = (duration - overlap_us + step - 1) / step;
    // The same length each: (duration + the overlaps) shared out, rounded up, never over the limit.
    let length = ((duration + (n - 1) * overlap_us + n - 1) / n).min(max_us);
    (0..n)
        .map(|k| {
            let start = k * (length - overlap_us);
            let end = if k + 1 == n { duration } else { start + length };
            RangeUs { start, end }
        })
        .collect()
}

/// Writes `src`'s copy into `out_dir` as `chunk-00.mp4`, `chunk-01.mp4`... (`CHUNK_US` long at most, overlapping by
/// `OVERLAP_US`), its picture fitted in `short_side` (360: 640 × 360 or 360 × 640). Blocking
/// (run it off the async threads); `cancel` stops it between packets.
pub fn make(
    src: &Path,
    out_dir: &Path,
    short_side: u32,
    cancel: &AtomicBool,
) -> Result<ProxyInfo, VideoError> {
    make_in_chunks(src, out_dir, short_side, (CHUNK_US, OVERLAP_US), cancel)
}

/// `make` with chunks of at most `max_us` overlapping by `overlap_us` (tests use short ones).
pub fn make_in_chunks(
    src: &Path,
    out_dir: &Path,
    short_side: u32,
    (max_us, overlap_us): (Us, Us),
    cancel: &AtomicBool,
) -> Result<ProxyInfo, VideoError> {
    let clip = probe(src)?;
    let duration_us = secs_to_us(clip.duration);
    std::fs::create_dir_all(out_dir).map_err(|e| VideoError::Failed(format!("folder: {e}")))?;
    // The long side in proportion: 640 for 360.
    let fit = (short_side * FIT.0 / FIT.1, short_side);
    let mut chunks = Vec::new();
    let mut found = Vec::new();
    // The copy's facts, from its first chunk (every chunk is made the same way).
    let mut first = None;
    for (k, range) in plan(duration_us, max_us, overlap_us)
        .into_iter()
        .enumerate()
    {
        let path = out_dir.join(format!("chunk-{k:02}.mp4"));
        let spec = SmallCopy {
            range,
            fit,
            max_fps: MAX_FPS,
            video_bps: VIDEO_BPS,
            audio_bps: AUDIO_BPS,
        };
        // Every frame of the source, at its own rate, into this chunk's finder (told when the copy starts again).
        let mut finder = Finder::new();
        let info =
            small_copy::write(src, &path, &spec, cancel, Some(&mut finder)).map_err(video_error)?;
        found.push((range, finder.finish()));
        chunks.push(ProxyChunk {
            path,
            range,
            bytes: info.bytes,
        });
        first.get_or_insert(info);
    }
    let made = first.ok_or_else(|| VideoError::Failed("no chunk".into()))?;
    Ok(ProxyInfo {
        duration_us,
        audio_lead_us: clip.audio_lead_us,
        width: made.width,
        height: made.height,
        fps: made.fps,
        has_audio: made.has_audio,
        chunks,
        cuts: cuts::merge(found),
    })
}

/// A small MP4 made already (by an earlier run, or a test's fixture) as a one-chunk copy, used as it is: it must be at
/// most `CHUNK_US` long. Its cuts are found by decoding it at its own rate, so it should keep every frame. Blocking.
pub fn existing(file: &Path) -> Result<ProxyInfo, VideoError> {
    let clip = probe(file)?;
    let duration_us = secs_to_us(clip.duration);
    if duration_us > CHUNK_US {
        return Err(VideoError::Failed(
            "a ready copy is at most ten minutes long".into(),
        ));
    }
    let bytes = std::fs::metadata(file)
        .map_err(|e| VideoError::Failed(format!("size: {e}")))?
        .len();
    let cuts = cuts::detect(file, &AtomicBool::new(false))?;
    Ok(ProxyInfo {
        duration_us,
        audio_lead_us: clip.audio_lead_us,
        width: clip.width,
        height: clip.height,
        fps: clip.fps,
        has_audio: clip.has_audio,
        chunks: vec![ProxyChunk {
            path: file.to_path_buf(),
            range: RangeUs {
                start: 0,
                end: duration_us,
            },
            bytes,
        }],
        cuts,
    })
}

/// What the engine's probe says of a video file.
fn probe(file: &Path) -> Result<MediaInfo, VideoError> {
    let clip = probe_media(file).map_err(VideoError::Failed)?;
    if clip.kind != MediaKind::Video {
        return Err(VideoError::NoVideo);
    }
    Ok(clip)
}

fn video_error(e: SmallCopyError) -> VideoError {
    match e {
        SmallCopyError::NoVideo => VideoError::NoVideo,
        SmallCopyError::NoEncoder => VideoError::NoEncoder,
        SmallCopyError::Stopped => VideoError::Stopped,
        SmallCopyError::Failed(e) => VideoError::Failed(e),
    }
}
