//! An editing copy's job: a folder `<proxies root>/<stem>.job/` beside the finished copies, so a paused,
//! crashed, quit or full-disk copy continues where it stopped. It holds `proxy.json` (the plan, each 10 s segment's
//! state, the job's state) and the segments `seg-00042.mp4`, then `joined.mp4` until it is renamed to
//! `<proxies root>/<stem>.mp4`. The stem hashes the source's path, size and modification time and the copy's size
//! (`key::stem`), so an edited or replaced file never finds an old copy or job.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_model::time::Us;
use klipolo_platform::durable::{self, Durability};
use serde::{Deserialize, Serialize};

use super::decide::{ProxyPolicy, proxy_bitrate, proxy_rate, proxy_size};
use super::facts::{ProbeError, SourceFacts, probe_facts};
use crate::export::job::{PartStatus, Segment};
use crate::export::segment::{SegmentLedger, verify};
use crate::export::shared::{FrameRate, WorkerError};
use crate::media::key::{CacheKind, SourceId, stem, stem_of};

pub use super::record::{CopyRecord, record_path, remove_copy};

/// Segment length: a crash, quit or pause costs at most this much copying.
pub const PROXY_SEGMENT_SECONDS: u32 = 10;
/// The manifest's name in the job folder.
pub const PROXY_FILE: &str = "proxy.json";
/// The joined copy in the job folder, until it is renamed into place.
pub const JOINED_FILE: &str = "joined.mp4";
const VERSION: u32 = 2;

/// `<root>/<stem>.job`.
pub fn job_dir(root: &Path, stem: &str) -> PathBuf {
    root.join(format!("{stem}.job"))
}

/// `<root>/<stem>.mp4`, the finished copy.
pub fn proxy_path(root: &Path, stem: &str) -> PathBuf {
    root.join(format!("{stem}.mp4"))
}

/// The thumbnails folder the copy refills (`Fill::Full`): `thumbs` beside the proxies folder (the app keeps both in
/// its media cache: `media/proxies` and `media/thumbs`).
pub fn thumbs_root_for(root: &Path) -> PathBuf {
    root.parent().unwrap_or(root).join("thumbs")
}

/// The job as a whole, as its last writer left it (a worker's run is `Running`; a crash leaves it so).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProxyState {
    Running,
    Paused,
    Failed,
}

/// Whether the source is still the file the job was planned for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceState {
    Same,
    /// Edited or replaced (another size or modification time).
    Changed,
    /// Gone or unreadable.
    Missing,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyJob {
    pub version: u32,
    /// The source file's path.
    pub source: String,
    pub stem: String,
    /// The policy's long side the stem was made with (`CacheKind::Proxy`).
    pub long_side: u32,
    /// `<root>/<stem>.mp4`.
    pub out: PathBuf,
    /// The copy's size, and its rate: the source's own, exactly (`decide::proxy_rate`), so copy frame n is source frame n.
    pub width: u32,
    pub height: u32,
    pub rate: FrameRate,
    pub total_frames: u64,
    /// About `PROXY_SEGMENT_SECONDS` of frames.
    pub segment_frames: u64,
    pub bitrate: u32,
    /// The source as planned: its shown size, exact rate and length (the worker's frame plans use them).
    pub source_width: u32,
    pub source_height: u32,
    pub source_fps: f64,
    pub duration_us: Us,
    /// The source file the job was planned from (its size, time and number): with the facts above, what the stem binds.
    pub source_id: SourceId,
    /// Where the source's thumbnail sprites live, and its stem there (None: no thumbnails are refilled).
    pub thumbs_root: PathBuf,
    pub thumbs_stem: Option<String>,
    /// Its segments are numbered from the source's video stream's start; false in an older job.
    #[serde(default)]
    pub from_stream_start: bool,
    pub segments: Vec<Segment>,
    pub state: ProxyState,
    pub message: Option<String>,
    #[serde(skip)]
    pub dir: PathBuf,
}

/// What `ProxyJob::open_for` found for a source.
#[derive(Debug)]
pub enum Opened {
    /// No copy and no job yet: a new job, planned and written.
    Fresh(ProxyJob),
    /// An unfinished job: its finished segments are kept.
    Resumed(ProxyJob),
    /// The finished copy.
    Done(PathBuf),
    /// The source is there but can't be read as video (damaged, not a video): no copy can be made of it.
    Stale,
    /// No job now, for a coded reason that says whether and when to try again (`Refused::retry`). Never `Unreadable`:
    /// that is `Stale`. A source that isn't there now (an unplugged drive) is `SourceUnavailable`, not `Stale`.
    Refused(Refused),
}

/// Why no job could be planned or written, coded: the scheduler words each one and retries by `retry`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The source is there but can't be read as video (damaged, not a video).
    Unreadable,
    /// The source isn't there now: its drive is unplugged, its volume isn't mounted, or it was moved or deleted.
    SourceUnavailable,
    /// Reading the source took too long (a stalled network share or drive).
    SourceTimedOut,
    /// Neither the file nor a read of its packets says how long it is: a copy would show one picture.
    UnknownLength,
    /// The file states no usable frame rate (none, or past `decide::MAX_COPY_FPS`).
    UnusualRate,
    /// The file changed while it was being looked at (it is being written or replaced).
    Changing,
    /// The disk is almost full.
    DiskFull,
    /// The cache folder can't be written (read-only, no permission, a failing drive): the OS's reason.
    CacheUnwritable(String),
}

/// When to try a refused copy (or a failed run) again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retry {
    /// Not by itself: only when the user asks.
    Never,
    /// Later, waiting longer each time.
    Backoff,
    /// Once free space is back above the floor (`run::FREE_FLOOR`).
    WhenSpaceFrees,
    /// When the source is there again (its volume mounted), or the next time the project's files are listed.
    WhenPresent,
}

impl Refused {
    pub fn retry(&self) -> Retry {
        match self {
            Self::SourceTimedOut | Self::Changing => Retry::Backoff,
            Self::DiskFull => Retry::WhenSpaceFrees,
            Self::SourceUnavailable => Retry::WhenPresent,
            Self::Unreadable
            | Self::UnknownLength
            | Self::UnusualRate
            | Self::CacheUnwritable(_) => Retry::Never,
        }
    }

    /// A file error writing the job: a full disk, or a cache that can't be written (with the OS's reason).
    pub fn from_io(e: &std::io::Error) -> Self {
        match durable::classify(e) {
            durable::DiskReason::Full => Self::DiskFull,
            _ => Self::CacheUnwritable(e.to_string()),
        }
    }
}

impl From<ProbeError> for Refused {
    fn from(e: ProbeError) -> Self {
        match e {
            ProbeError::TimedOut => Self::SourceTimedOut,
            ProbeError::Changing => Self::Changing,
            ProbeError::Unreadable(_) => Self::Unreadable,
        }
    }
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable => f.write_str("This file can't be read."),
            Self::SourceUnavailable => {
                f.write_str("This file isn't available now. Is its drive connected?")
            }
            Self::SourceTimedOut => f.write_str("This file took too long to read."),
            Self::UnknownLength => f.write_str("This file doesn't say how long it is."),
            Self::UnusualRate => f.write_str("This file doesn't state a usable frame rate."),
            Self::Changing => f.write_str("This file changed while it was being read."),
            Self::DiskFull => f.write_str("Your disk is almost full."),
            Self::CacheUnwritable(reason) => f.write_str(&super::run::unwritable_message(reason)),
        }
    }
}

impl std::error::Error for Refused {}

fn read_error(e: impl std::fmt::Display) -> String {
    format!("This editing copy's progress couldn't be read: {e}")
}

impl ProxyJob {
    /// Plans a copy of `source` (`facts` probed from it) under `policy` in `<root>/<stem>.job/` and writes it. A folder
    /// already there (a job whose manifest was never written) is reused: every segment starts pending. A source of
    /// unknown length is refused (`UnknownLength`), never planned as a one-frame copy.
    pub fn create(
        root: &Path,
        source: &str,
        facts: &SourceFacts,
        policy: &ProxyPolicy,
    ) -> Result<Self, Refused> {
        let long_side = policy.long_side;
        let meta = std::fs::metadata(source).map_err(|_| Refused::SourceUnavailable)?;
        // The stem and the facts describe one version of the file: facts read from another (it was overwritten since)
        // would plan the wrong length, rate or shape under the new file's name.
        let source_id = SourceId::of(&meta);
        if facts.id.is_some_and(|id| id != source_id) {
            return Err(Refused::Changing);
        }
        let stem = stem_of(source, &meta, &CacheKind::Proxy { long_side });
        let (width, height) = proxy_size(facts, policy);
        let rate = proxy_rate(facts).ok_or(Refused::UnusualRate)?;
        let total_frames = rate.frames_in(facts.duration_us);
        if total_frames == 0 {
            return Err(Refused::UnknownLength);
        }
        let segment_frames =
            ((f64::from(PROXY_SEGMENT_SECONDS) * rate.f64()).round() as u64).max(1);
        let segments = (0..total_frames.div_ceil(segment_frames))
            .map(|i| {
                let start_frame = i * segment_frames;
                Segment {
                    index: i as u32,
                    start_frame,
                    frames: segment_frames.min(total_frames - start_frame),
                    file: format!("seg-{i:05}.mp4"),
                    bytes: 0,
                    checksum: 0,
                    status: PartStatus::Pending,
                    config: None,
                    software: false,
                }
            })
            .collect();
        let job = Self {
            version: VERSION,
            source: source.to_owned(),
            out: proxy_path(root, &stem),
            dir: job_dir(root, &stem),
            stem,
            long_side,
            width,
            height,
            rate,
            total_frames,
            segment_frames,
            bitrate: proxy_bitrate(width, height, rate.f64()),
            source_width: facts.width,
            source_height: facts.height,
            source_fps: facts.fps(),
            duration_us: facts.duration_us,
            source_id,
            thumbs_root: thumbs_root_for(root),
            thumbs_stem: stem_of_thumbs(source),
            from_stream_start: true,
            segments,
            state: ProxyState::Paused,
            message: None,
        };
        std::fs::create_dir_all(&job.dir)
            .and_then(|()| job.save())
            .map_err(|e| {
                tracing::warn!(%e, "editing copy: the job folder couldn't be written");
                Refused::from_io(&e)
            })?;
        Ok(job)
    }

    /// Reads the job in `dir`.
    pub fn open(dir: &Path) -> Result<Self, String> {
        let text = std::fs::read(dir.join(PROXY_FILE)).map_err(read_error)?;
        let mut job: Self = serde_json::from_slice(&text).map_err(read_error)?;
        if job.version != VERSION {
            return Err("This editing copy came from another version of Klipolo.".into());
        }
        if job.rate.num == 0
            || job.rate.den == 0
            || job.segment_frames == 0
            || job.total_frames == 0
            || job.segments.is_empty()
        {
            return Err(read_error("no frame rate or no segments"));
        }
        job.dir = dir.to_path_buf();
        Ok(job)
    }

    /// Today's state of `source`'s copy under `policy` (its stem now): the finished copy, an unfinished job, a new job
    /// (its facts probed here), `Stale` when the file is there but can't be read as video, or `Refused` with the reason
    /// no job can be made now (the source isn't there now, a full disk, a slow source, an unknown length, a cache that
    /// can't be written). A copy or job made for an
    /// older stem is never looked up: the stem changed with the file.
    pub fn open_for(root: &Path, source: &str, policy: &ProxyPolicy) -> Opened {
        Self::open_with(root, source, None, policy)
    }

    /// `open_for` with the source's facts already probed (`facts`), so a new job doesn't probe it again.
    pub fn open_with(
        root: &Path,
        source: &str,
        facts: Option<&SourceFacts>,
        policy: &ProxyPolicy,
    ) -> Opened {
        let Some(stem) = stem(
            source,
            &CacheKind::Proxy {
                long_side: policy.long_side,
            },
        ) else {
            // It can't be looked at now: an unplugged drive, a volume not mounted (tried again when it is back).
            return Opened::Refused(Refused::SourceUnavailable);
        };
        let out = proxy_path(root, &stem);
        if out.is_file() {
            return Opened::Done(out);
        }
        let dir = job_dir(root, &stem);
        // An older job of a late picture starts again in its folder (`create`), every segment to do.
        if dir.is_dir()
            && let Ok(job) = Self::open(&dir)
            && !super::numbering::restarts(&job, facts)
        {
            return Opened::Resumed(job);
        }
        let probed;
        let facts = match facts {
            Some(f) => f,
            None => match probe_facts(Path::new(source)) {
                Ok(f) => {
                    probed = f;
                    &probed
                }
                // Gone between the look and the read: not there now, rather than unreadable.
                Err(_) if !Path::new(source).is_file() => {
                    return Opened::Refused(Refused::SourceUnavailable);
                }
                Err(e) => return Self::refused(Refused::from(e)),
            },
        };
        match Self::create(root, source, facts, policy) {
            Ok(job) => Opened::Fresh(job),
            Err(e) => Self::refused(e),
        }
    }

    fn refused(reason: Refused) -> Opened {
        match reason {
            Refused::Unreadable => Opened::Stale,
            reason => Opened::Refused(reason),
        }
    }

    /// Every finished segment verified in full (size, CRC, frame count and duration), once a joined copy failed its
    /// check: the damaged ones are set to pending (and saved). Returns their indexes.
    pub fn verify_all(&mut self) -> Vec<u32> {
        let mut redo = Vec::new();
        for s in self
            .segments
            .iter_mut()
            .filter(|s| s.status == PartStatus::Done)
        {
            let path = self.dir.join(&s.file);
            if verify(&path, s.frames, self.rate, Some((s.bytes, s.checksum))).is_err() {
                s.status = PartStatus::Pending;
                redo.push(s.index);
            }
        }
        if !redo.is_empty()
            && let Err(e) = self.save()
        {
            tracing::warn!(%e, "editing copy: the job couldn't be saved");
        }
        redo
    }

    /// Before a run continues: every finished segment's file must still be there at its recorded size (a stat each),
    /// and the last one finished (the one a crash or a full disk could have cut) is verified again in full: size, CRC,
    /// frame count and duration. Earlier ones were fsynced and verified before they counted, so they aren't re-read.
    /// One that fails is set to pending (and saved). Returns their indexes.
    pub fn recheck(&mut self) -> Vec<u32> {
        let last = self
            .segments
            .iter()
            .filter(|s| s.status == PartStatus::Done)
            .map(|s| s.index)
            .max();
        let mut redo = Vec::new();
        for s in self
            .segments
            .iter_mut()
            .filter(|s| s.status == PartStatus::Done)
        {
            let path = self.dir.join(&s.file);
            let ok = if Some(s.index) == last {
                verify(&path, s.frames, self.rate, Some((s.bytes, s.checksum))).is_ok()
            } else {
                std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == s.bytes)
            };
            if !ok {
                s.status = PartStatus::Pending;
                redo.push(s.index);
            }
        }
        if !redo.is_empty()
            && let Err(e) = self.save()
        {
            tracing::warn!(%e, "editing copy: the job couldn't be saved");
        }
        redo
    }

    /// Writes the manifest atomically (an ordered sync: after every segment written before it).
    pub fn save(&self) -> std::io::Result<()> {
        // Compact: it is saved after every segment (an hour's job holds 360 of them).
        let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        durable::write_atomic(&self.dir.join(PROXY_FILE), &bytes, Durability::Ordered)
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.dir.join(relative)
    }

    /// The manifest the worker is started with.
    pub fn manifest_path(&self) -> PathBuf {
        self.dir.join(PROXY_FILE)
    }

    /// Segments written and verified.
    pub fn done_segments(&self) -> usize {
        self.segments
            .iter()
            .filter(|s| s.status == PartStatus::Done)
            .count()
    }

    /// Frames of the finished segments.
    pub fn frames_done(&self) -> u64 {
        self.segments
            .iter()
            .filter(|s| s.status == PartStatus::Done)
            .map(|s| s.frames)
            .sum()
    }

    /// The first segment still to render.
    pub fn next_pending(&self) -> Option<u32> {
        self.segments
            .iter()
            .find(|s| s.status != PartStatus::Done)
            .map(|s| s.index)
    }

    /// About how many bytes segment `index` takes (its bitrate's 1.5× peak, so the space check errs on the safe side).
    pub fn expected_bytes(&self, index: u32) -> u64 {
        let frames = self
            .segments
            .get(index as usize)
            .map_or(self.segment_frames, |s| s.frames);
        (f64::from(self.bitrate) * 1.5 * self.rate.seconds(frames) / 8.0) as u64
    }

    /// Records segment `index` as written and verified (its size and CRC, and the CRC of the encoder's parameter sets
    /// when known). Finished segments from an encoder with other parameter sets can't join with it: they are set back
    /// to pending (their frames are returned). Not saved: the caller saves.
    pub fn segment_done(
        &mut self,
        index: u32,
        (bytes, checksum): (u64, u32),
        config: Option<u32>,
    ) -> u64 {
        let mut set_back = 0;
        for s in &mut self.segments {
            if s.index == index {
                s.bytes = bytes;
                s.checksum = checksum;
                s.status = PartStatus::Done;
                s.config = config;
            } else if s.status == PartStatus::Done
                && config.is_some()
                && s.config.is_some()
                && s.config != config
            {
                s.status = PartStatus::Pending;
                set_back += s.frames;
            }
        }
        set_back
    }

    /// Whether the source is still the file this job copies (same path, size and modification time).
    pub fn source_state(&self) -> SourceState {
        match stem(
            &self.source,
            &CacheKind::Proxy {
                long_side: self.long_side,
            },
        ) {
            None => SourceState::Missing,
            Some(s) if s == self.stem => SourceState::Same,
            Some(_) => SourceState::Changed,
        }
    }
}

fn stem_of_thumbs(source: &str) -> Option<String> {
    stem(source, &CacheKind::Thumbs)
}

/// One segment the encoder thread verified, waiting for the run loop to record it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verified {
    pub index: u32,
    /// Size and CRC-32.
    pub file: (u64, u32),
    /// CRC-32 of the encoder's parameter sets.
    pub config: u32,
}

/// The editing copy's `SegmentLedger`. The encoder thread's verifier hands each written, fsynced and verified segment
/// here; the run loop (`run::run_segments`, the manifest's one writer) takes it and saves it atomically.
#[derive(Clone)]
pub struct ProxyJobHandle {
    dir: PathBuf,
    files: Arc<[String]>,
    verified: Arc<Mutex<Vec<Verified>>>,
}

impl ProxyJobHandle {
    pub fn new(job: &ProxyJob) -> Self {
        Self {
            dir: job.dir.clone(),
            files: job.segments.iter().map(|s| s.file.clone()).collect(),
            verified: Arc::default(),
        }
    }

    /// Segment `index`, once verified.
    pub fn take(&self, index: u32) -> Option<Verified> {
        let mut verified = self.verified.lock().unwrap_or_else(PoisonError::into_inner);
        let at = verified.iter().position(|v| v.index == index)?;
        Some(verified.swap_remove(at))
    }
}

impl SegmentLedger for ProxyJobHandle {
    fn segment_path(&self, _output: usize, index: u32) -> PathBuf {
        let file = self
            .files
            .get(index as usize)
            .cloned()
            .unwrap_or_else(|| format!("seg-{index:05}.mp4"));
        self.dir.join(file)
    }

    fn segment_done(
        &self,
        _output: usize,
        index: u32,
        file: (u64, u32),
        (config, _software): (u32, bool),
    ) -> Result<u64, WorkerError> {
        self.verified
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Verified {
                index,
                file,
                config,
            });
        Ok(0)
    }
}
