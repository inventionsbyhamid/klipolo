//! Following the speaker in a vertical or square version: a tracker (`providers::SubjectTracker`, SAM through the Meta
//! adapter) finds every person on a small, low frame rate copy of only the footage that plays, and `path` turns its
//! boxes into the crop's path for one format.
//!
//! - **What is sent:** only the stretches of a file the clips play (`requests`: stretches at most `JOIN_GAP_US` apart go
//!   as one, so footage that was cut out never goes up; none longer than the tracker takes), each as an MP4 of `FPS`
//!   frames a second at most `COPY_FIT` (`SubjectCopy`; the engine's small copy, with the cut finder watching every
//!   source frame on the way, so the shots come with it).
//! - **The price** comes first (`estimate_usd`): each request's frames at the tracker's price. What the cache covers
//!   costs nothing: only the parts it doesn't (`cover`, `missing`) are sent.
//! - **The cache:** each file's tracked stretches (`SubjectTrack`) are kept in its analysis cache folder
//!   (`subjects.json`), by model, phrase, frame rate and stretch: any part tracked before is read from there, from as
//!   many stretches as cover it, and never paid for again. A failed or stopped request stores nothing; an answer cut off
//!   part-way stores only the frames it holds, so asking again pays only for the rest.
//! - **Times:** frame n of a stretch is at its start plus n / `fps` on the file's clock; the cuts are on that clock too.

pub mod clips;
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
/// Stretches of one file at most this far apart go as one request (at most two copy frames that don't play).
pub const JOIN_GAP_US: Us = 1_000_000;
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
    /// Whether it is `model`'s answer for `phrase` at `fps`.
    pub fn serves(&self, model: &str, phrase: &str, fps: f64) -> bool {
        self.model == model && self.phrase == phrase && (self.fps - fps).abs() < 1e-9
    }

    /// Whether it is that answer and holds every frame of `range`.
    pub fn covers(&self, model: &str, phrase: &str, fps: f64, range: RangeUs) -> bool {
        self.serves(model, phrase, fps)
            && self.range.start <= range.start
            && range.end <= self.range.end
    }
}

/// The tracked stretches that cover a stretch asked for, and what tracking the missing parts cost now (0 when the
/// cache held all of it).
#[derive(Clone, Debug, PartialEq)]
pub struct Tracked {
    pub tracks: Vec<SubjectTrack>,
    pub paid_usd: f64,
}

/// What the cache holds of a stretch: the tracked stretches that overlap it, and its parts none of them covers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cover {
    pub tracks: Vec<SubjectTrack>,
    pub missing: Vec<RangeUs>,
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

/// What the cache holds of `range` for `model`'s answer to `phrase` at `fps`. Blocking (a small read).
pub fn cover(dir: &Dir, model: &str, phrase: &str, fps: f64, range: RangeUs) -> Cover {
    let kept: Vec<SubjectTrack> = dir.load(CACHE_NAME).unwrap_or_default();
    let tracks: Vec<SubjectTrack> = (kept.into_iter())
        .filter(|t| {
            t.serves(model, phrase, fps) && t.range.start < range.end && range.start < t.range.end
        })
        .collect();
    let covered: Vec<RangeUs> = tracks.iter().map(|t| t.range).collect();
    Cover {
        missing: missing(range, &covered, fps),
        tracks,
    }
}

/// The parts of `range` none of `covered` holds, in order; a part shorter than one frame at `fps` counts as held.
pub fn missing(range: RangeUs, covered: &[RangeUs], fps: f64) -> Vec<RangeUs> {
    let frame = (1e6 / fps.max(0.001)) as Us;
    let mut held: Vec<RangeUs> = covered.to_vec();
    held.sort_by_key(|r| r.start);
    let mut out = Vec::new();
    let mut at = range.start;
    for r in held {
        if r.end <= at || r.start >= range.end {
            continue;
        }
        if r.start > at {
            out.push(RangeUs {
                start: at,
                end: r.start.min(range.end),
            });
        }
        at = at.max(r.end);
    }
    if at < range.end {
        out.push(RangeUs {
            start: at,
            end: range.end,
        });
    }
    out.retain(|r| r.end - r.start >= frame);
    out
}

/// Adds `track` to the file's cached stretches, dropping those it covers.
fn store(dir: &Dir, track: &SubjectTrack) -> std::io::Result<()> {
    let mut tracks: Vec<SubjectTrack> = dir.load(CACHE_NAME).unwrap_or_default();
    tracks.retain(|t| !track.covers(&t.model, &t.phrase, t.fps, t.range));
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

/// `ask`'s stretch tracked: what the cache holds of it, and its missing parts each copied small, sent to `tracker` and
/// cached (a cache that can't be written only costs a second request later). An answer cut off part-way keeps, and
/// caches, only the frames it holds.
pub async fn track(
    tracker: &dyn SubjectTracker,
    copier: Arc<dyn SubjectCopy>,
    ask: &Ask<'_>,
) -> Result<Tracked, AnalysisError> {
    let held = match ask.cache {
        Some(dir) => {
            let (dir, model, phrase, fps, range) = (
                dir.clone(),
                tracker.model().to_owned(),
                ask.phrase.to_owned(),
                ask.fps,
                ask.range,
            );
            tokio::task::spawn_blocking(move || cover(&dir, &model, &phrase, fps, range))
                .await
                .unwrap_or_default()
        }
        None => Cover::default(),
    };
    let missing = match ask.cache {
        Some(_) => held.missing,
        None => vec![ask.range],
    };
    let mut tracked = Tracked {
        tracks: held.tracks,
        paid_usd: 0.0,
    };
    for part in missing {
        let track = track_part(tracker, copier.clone(), ask, part).await?;
        tracked.paid_usd += track.cost_usd;
        // A whole answer with nobody in it is kept too: that stretch is never paid for again.
        if track.range.end > track.range.start {
            if let Some(dir) = ask.cache {
                let (dir, kept) = (dir.clone(), track.clone());
                let _ = tokio::task::spawn_blocking(move || store(&dir, &kept)).await;
            }
            tracked.tracks.push(track);
        }
    }
    Ok(tracked)
}

/// `part` of `ask`'s file copied small and tracked, its range cut back to what an answer cut off part-way holds.
async fn track_part(
    tracker: &dyn SubjectTracker,
    copier: Arc<dyn SubjectCopy>,
    ask: &Ask<'_>,
    part: RangeUs,
) -> Result<SubjectTrack, AnalysisError> {
    std::fs::create_dir_all(ask.work)
        .map_err(|e| VideoError::Failed(format!("working folder: {e}")))?;
    let copy = Copy(ask.work.join(format!("subject-{}.mp4", unique_name())));
    let cuts = {
        let (copier, src, out, fps, cancel) = (
            copier,
            ask.src.to_path_buf(),
            copy.0.clone(),
            ask.fps,
            ask.cancel.clone(),
        );
        tokio::task::spawn_blocking(move || copier.copy(&src, part, fps, &out, &cancel))
            .await
            .map_err(|e| VideoError::Failed(format!("copy: {e}")))??
    };
    let tracking = tracker.track(&copy.0, ask.phrase).await?;
    drop(copy);
    let mut range = part;
    if tracking.cut_off {
        let last = tracking
            .frames
            .iter()
            .map(|f| f.frame)
            .max()
            .map_or(0, |f| f + 1);
        range.end = (part.start + (f64::from(last) / ask.fps * 1e6).round() as Us).min(part.end);
    }
    Ok(SubjectTrack {
        model: tracker.model().to_owned(),
        phrase: ask.phrase.to_owned(),
        fps: ask.fps,
        range,
        frames: tracking.frames,
        cuts: (cuts.into_iter())
            .filter(|c| (range.start..range.end).contains(c))
            .collect(),
        frames_billed: tracking.frames_billed,
        cost_usd: tracking.cost_usd,
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
