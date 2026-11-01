//! Following the speaker in a vertical or square version: a tracker (`providers::SubjectTracker`, SAM through the Meta
//! adapter) finds every person on a small, low frame rate copy of only the footage that plays, and `path` turns its
//! boxes into the crop's path for one format.
//!
//! - **What is sent:** the stretches of a file the clips play (`requests`: stretches less than `JOIN_GAP_US` apart go
//!   as one, so a cut doesn't cost a second request's minimum; none longer than the tracker takes), each as an MP4 of
//!   `FPS` frames a second at most `COPY_FIT` (`SubjectCopy`; the engine's small copy, with the cut finder watching
//!   every source frame on the way, so the shots come with it).
//! - **The price** comes first (`estimate_usd`): each request's frames at the tracker's price. A stretch the cache
//!   covers costs nothing.
//! - **The cache:** each file's tracked stretches (`SubjectTrack`) are kept in its analysis cache folder
//!   (`subjects.json`), by phrase, frame rate and stretch: any part of a stretch tracked before is read from there and
//!   never paid for again. A failed or stopped request stores nothing.
//! - **Times:** frame n of a stretch is at its start plus n / `fps` on the file's clock; the cuts are on that clock too.

pub mod path;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use klipolo_engine::media::small_copy::{self, SmallCopy, SmallCopyError};
use klipolo_model::time::{RangeUs, Us};
use serde::{Deserialize, Serialize};

use crate::cache::Dir;
use crate::cuts::Finder;
use crate::error::{AnalysisError, VideoError};
use crate::providers::{SubjectTracker, TrackedFrame};

/// Frames a second sent: a speaker moves little in half a second, and a 60 s short is 120 frames.
pub const FPS: f64 = 2.0;
/// The phrase that finds speakers.
pub const PERSON: &str = "person";
/// The box a copy's picture fits in (long side, short side): 480 lines keep small faces and far people trackable.
pub const COPY_FIT: (u32, u32) = (854, 480);
/// The copy's picture bitrate: a few frames a second at 480 lines.
const COPY_BPS: u32 = 300_000;
/// Stretches of one file closer than this go as one request.
pub const JOIN_GAP_US: Us = 10_000_000;
/// The cache file of a file's tracked stretches.
const CACHE_NAME: &str = "subjects";

/// A tracked stretch of a file, as the cache keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubjectTrack {
    /// The tracker's model.
    pub model: String,
    pub phrase: String,
    pub fps: f64,
    /// The stretch, on the file's clock: frame n is at `range.start + n / fps`.
    pub range: RangeUs,
    pub frames: Vec<TrackedFrame>,
    /// The stretch's hard cuts on the file's clock, each the first frame of a new shot.
    pub cuts: Vec<Us>,
    pub frames_billed: u64,
    pub cost_usd: f64,
}

impl SubjectTrack {
    /// Whether it holds every frame of `range` for `phrase` at `fps`.
    pub fn covers(&self, phrase: &str, fps: f64, range: RangeUs) -> bool {
        self.phrase == phrase
            && (self.fps - fps).abs() < 1e-9
            && self.range.start <= range.start
            && range.end <= self.range.end
    }
}

/// A tracked stretch, and what tracking it cost now (0 when it came from the cache).
#[derive(Clone, Debug, PartialEq)]
pub struct Tracked {
    pub track: SubjectTrack,
    pub paid_usd: f64,
}

/// The local work: a stretch of a file as the small video the tracker takes, with its cuts. Behind a trait so tests
/// stand in for the engine (Linux has no H.264 encoder).
pub trait SubjectCopy: Send + Sync {
    /// `range` of `src` (its file clock) at `fps` frames a second, written to `out` as an MP4; gives the stretch's hard
    /// cuts on the file's clock. Blocking; `cancel` stops it.
    fn copy(
        &self,
        src: &Path,
        range: RangeUs,
        fps: f64,
        out: &Path,
        cancel: &AtomicBool,
    ) -> Result<Vec<Us>, VideoError>;
}

/// The engine's small copy, with no sound, its cut finder watching every source frame.
pub struct EngineCopy;

impl SubjectCopy for EngineCopy {
    fn copy(
        &self,
        src: &Path,
        range: RangeUs,
        fps: f64,
        out: &Path,
        cancel: &AtomicBool,
    ) -> Result<Vec<Us>, VideoError> {
        let spec = SmallCopy {
            range,
            fit: COPY_FIT,
            max_fps: fps,
            video_bps: COPY_BPS,
            audio_bps: 0,
            threads: small_copy::background_threads(
                std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
            ),
        };
        let mut finder = Finder::new();
        small_copy::write(src, out, &spec, cancel, Some(&mut finder)).map_err(|e| match e {
            SmallCopyError::NoVideo => VideoError::NoVideo,
            SmallCopyError::NoEncoder => VideoError::NoEncoder,
            SmallCopyError::Stopped => VideoError::Stopped,
            SmallCopyError::Failed(e) => VideoError::Failed(e),
        })?;
        Ok(finder.finish())
    }
}

/// The frames a stretch of `len_us` is sent as at `fps`: at least one.
pub fn frames_for(len_us: Us, fps: f64) -> u64 {
    ((len_us.max(0) as f64 / 1e6 * fps).ceil() as u64).max(1)
}

/// The requests that track `ranges` of one file: in order, stretches less than `JOIN_GAP_US` apart joined, and each
/// cut to at most `max_frames` frames.
pub fn requests(ranges: &[RangeUs], fps: f64, max_frames: u64) -> Vec<RangeUs> {
    let mut sorted: Vec<RangeUs> = ranges.iter().copied().filter(|r| r.end > r.start).collect();
    sorted.sort_by_key(|r| r.start);
    let mut joined: Vec<RangeUs> = Vec::new();
    for r in sorted {
        match joined.last_mut() {
            Some(last) if r.start <= last.end + JOIN_GAP_US => last.end = last.end.max(r.end),
            _ => joined.push(r),
        }
    }
    let longest = ((max_frames.max(1) as f64 / fps) * 1e6) as Us;
    let mut out = Vec::new();
    for r in joined {
        let mut start = r.start;
        while start < r.end {
            let end = (start + longest).min(r.end);
            out.push(RangeUs { start, end });
            start = end;
        }
    }
    out
}

/// What tracking `ranges` of one file costs before any call: each request's frames at `tracker`'s price.
pub fn estimate_usd(tracker: &dyn SubjectTracker, ranges: &[RangeUs]) -> f64 {
    requests(ranges, FPS, tracker.max_frames())
        .iter()
        .map(|r| tracker.estimate_usd(frames_for(r.end - r.start, FPS)))
        .sum()
}

/// A stretch the cache keeps that covers `range` for `phrase` at `fps`. Blocking (a small read).
pub fn cached(dir: &Dir, phrase: &str, fps: f64, range: RangeUs) -> Option<SubjectTrack> {
    let tracks: Vec<SubjectTrack> = dir.load(CACHE_NAME)?;
    tracks.into_iter().find(|t| t.covers(phrase, fps, range))
}

/// Adds `track` to the file's cached stretches, dropping those it covers.
fn store(dir: &Dir, track: &SubjectTrack) -> std::io::Result<()> {
    let mut tracks: Vec<SubjectTrack> = dir.load(CACHE_NAME).unwrap_or_default();
    tracks.retain(|t| !(track.covers(&t.phrase, t.fps, t.range)));
    tracks.push(track.clone());
    dir.store(CACHE_NAME, &tracks)
}

/// One stretch to track.
pub struct Ask<'a> {
    /// The file.
    pub src: &'a Path,
    /// Its analysis cache folder; None tracks without one.
    pub cache: Option<&'a Dir>,
    /// Where the small copy is made (removed once sent).
    pub work: &'a Path,
    /// The stretch, on the file's clock.
    pub range: RangeUs,
    pub phrase: &'a str,
    pub fps: f64,
    pub cancel: Arc<AtomicBool>,
}

/// `ask`'s stretch tracked: from the cache when a stretch kept there covers it, else copied small, sent to `tracker`
/// and cached (a cache that can't be written only costs a second request later).
pub async fn track(
    tracker: &dyn SubjectTracker,
    copier: Arc<dyn SubjectCopy>,
    ask: &Ask<'_>,
) -> Result<Tracked, AnalysisError> {
    if let Some(dir) = ask.cache
        && let Some(track) = cached(dir, ask.phrase, ask.fps, ask.range)
    {
        return Ok(Tracked {
            track,
            paid_usd: 0.0,
        });
    }
    std::fs::create_dir_all(ask.work)
        .map_err(|e| VideoError::Failed(format!("working folder: {e}")))?;
    let copy = Copy(ask.work.join(format!("subject-{}.mp4", unique_name())));
    let cuts = {
        let (copier, src, out, range, fps, cancel) = (
            copier,
            ask.src.to_path_buf(),
            copy.0.clone(),
            ask.range,
            ask.fps,
            ask.cancel.clone(),
        );
        tokio::task::spawn_blocking(move || copier.copy(&src, range, fps, &out, &cancel))
            .await
            .map_err(|e| VideoError::Failed(format!("copy: {e}")))??
    };
    let tracking = tracker.track(&copy.0, ask.phrase).await?;
    drop(copy);
    let track = SubjectTrack {
        model: tracker.model().to_owned(),
        phrase: ask.phrase.to_owned(),
        fps: ask.fps,
        range: ask.range,
        frames: tracking.frames,
        cuts: (cuts.into_iter())
            .filter(|c| (ask.range.start..ask.range.end).contains(c))
            .collect(),
        frames_billed: tracking.frames_billed,
        cost_usd: tracking.cost_usd,
    };
    if let Some(dir) = ask.cache {
        let (dir, kept) = (dir.clone(), track.clone());
        let _ = tokio::task::spawn_blocking(move || store(&dir, &kept)).await;
    }
    Ok(Tracked {
        paid_usd: track.cost_usd,
        track,
    })
}

/// The small copy's file, removed when dropped.
struct Copy(PathBuf);

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A name no other copy has: the time and a counter.
fn unique_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
        .map_or(0, |d| d.as_nanos());
    format!("{nanos:x}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}
