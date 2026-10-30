//! Decoder pool: one vendored decoder per (asset, lane), spawned on demand. A second lane opens only when one frame
//! needs the same asset at two different times (one decoder thread would otherwise thrash between them). `warm`
//! opens upcoming clips' and overlays' decoders in the background during playback, so a cut never waits for a reader
//! to open. Each decoder gets its role's cache budget (main, camera, overlay). Decoders drop their reader and cache
//! after the tuning's idle timeout; past `MAX_SLOTS` decoders, or past `POOL_CACHE_BYTES` of cache budgets (both
//! counting the ones still warming), the least recently used ones close (never one the current frame uses). A warm-up
//! that no frame claimed within `WARM_EXPIRY` (its clip was deleted, or playback seeked or stopped) is cancelled.
//! Under memory pressure the budget halves and nothing is warmed; when critical, only the current frame's decoders
//! stay open (`render::pressure`).
//!
//! A frame its decoder stalls on is recovered in place (`decoder`): the reader is opened again once at the
//! keyframe before it, then the frame is decoded in software, and only then is the file "couldn't be read".
//!
//! A source read from its editing copy (`SourceRef::path` ≠ `original`) whose copy can't be opened (its cache
//! was purged, or it is damaged) is opened from the original instead, and the app is told once
//! (`MediaEvent::Changed { kind: Proxy }`, naming the original) so it drops the mapping. A slot opened for another file
//! than the plan now names (a copy swapped in, or dropped) is opened again.

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_model::time::Us;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;

use crate::media::MediaSink;
use crate::plan::{FramePlan, VideoRole};
use crate::render::pressure;
pub use crate::render::probe::{probe_fps, probe_rotation};

mod decoder;
mod failure;
mod lanes;
mod open;
mod slots;
mod stat;
mod warm;
use crate::decode_wait::DecodeBeat;
use crate::vendor::cap::rendering::decoder::{DecodedFrame, DecoderTuning, Superseded};
pub use decoder::frame_shown;
use decoder::{Decoder, Read, Recovery, reopen_quiet};
use failure::Failure;
pub use failure::{missing, not_answering, unreadable};
use open::open;
#[cfg(any(test, feature = "trial"))]
pub use stat::slow_stats;
pub(crate) use stat::{Ask, FileState, file_id, file_state};

/// What a frame's video layers show, in the plan's layer order (reused frame to frame: no per-frame maps).
#[derive(Default)]
pub struct DecodedSet {
    /// Per video layer: its frame and the clockwise degrees that turn it upright (`probe_rotation`), or None when it
    /// couldn't be opened or decoded.
    pub frames: Vec<Option<(DecodedFrame, u16)>>,
    /// Files that are there but couldn't be read ("This file couldn't be read: intro.mp4").
    pub warnings: Vec<String>,
    /// Sources that aren't there (moved, renamed, their drive removed), by the original's path: the preview says each
    /// with the code an editing copy's row uses (`api::MEDIA_MISSING`); export fails on them (`missing`).
    pub missing: Vec<Arc<str>>,
    /// Sources whose disk didn't answer in time, by the original's path: the preview says nothing (its redraw fires
    /// when the answer comes); export stops, naming the first (`not_answering`).
    pub not_answering: Vec<Arc<str>>,
}

impl DecodedSet {
    /// The `index`-th video layer's frame and rotation.
    pub fn get(&self, index: usize) -> Option<&(DecodedFrame, u16)> {
        self.frames.get(index).and_then(Option::as_ref)
    }
}

enum State {
    Ready(Decoder),
    Failed { failure: Failure, at: Instant },
}

struct Slot {
    state: State,
    /// The plan's `path` it was opened for (a copy, or the source).
    path: Arc<str>,
    /// The pool's `tick` when a frame last asked for it.
    used: u64,
    /// The source time it was last asked for: its reader sits there (`lanes`).
    at_us: Us,
    /// The most its frame cache may hold (0 for a failed open).
    budget: usize,
    /// Its decoder's file didn't answer after a frame failed: the next frame asks that stat before the reader (still
    /// out, it answers at once), so no frame waits for the reader's timeout again while the disk is silent.
    silent: bool,
}

struct Warming {
    task: JoinHandle<Opened>,
    /// The plan's `path` it is opening.
    path: Arc<str>,
    budget: usize,
    since: Instant,
    /// The source time it is being opened at.
    at_us: Us,
}

/// Open decoders kept for scrubbing back and forth. Each may hold up to two readers, within the process-wide reader
/// limit (6 on Intel).
pub const MAX_SLOTS: usize = 6;
/// Overlay (b-roll) decoders cache at most this much; the camera gets half the main budget.
const OVERLAY_CACHE_BYTES: usize = 32 << 20;
/// The open decoders' cache budgets together (one ~256 MiB frame budget on Intel): a recording
/// (96 + 48 MiB) and a few overlays. Decoders past it close, least recently used first.
pub const POOL_CACHE_BYTES: usize = 256 << 20;
/// Preview decode sizes by role (oversize sources are decoded at preview size; export is full size).
const MAIN_BOX: (u32, u32) = (1920, 1080);
const SMALL_BOX: (u32, u32) = (1280, 720);
/// A warm-up nobody claimed in this long is for a start that never came (the lookahead is 1 s).
const WARM_EXPIRY: Duration = Duration::from_secs(3);

/// Cap wants `&'static str` thread names. Two lanes: one frame rarely shows one file at more than two times.
const NAMES: [&str; 2] = ["klipolo-video-0", "klipolo-video-1"];
/// A file that failed to open is tried again after this long (it may have been reconnected).
const RETRY_AFTER: Duration = Duration::from_secs(5);
/// How long the preview waits to hear whether a file is there (`stat`): a disk that takes longer (spinning up, a network
/// volume) is not answering yet, never missing, and the redraw signal fires when it answers (`set_redraw`).
const STAT_LIMIT: Duration = Duration::from_secs(1);
/// How long export waits to hear whether a file is there: far longer than a disk takes to spin up, and under the
/// parent's watchdog (`export::attempt::STALL`, 120 s), so a disk that never answers stops the export with its name
/// instead of a generic stall.
pub(crate) const EXPORT_STAT_LIMIT: Duration = Duration::from_secs(60);

type Opened = Result<Decoder, Failure>;

pub struct DecoderPool {
    slots: HashMap<(Arc<str>, u8), Slot>,
    /// Decoders `warm` is opening; `slot` waits for one instead of opening the file twice.
    warming: HashMap<(Arc<str>, u8), Warming>,
    tuning: DecoderTuning,
    force_ffmpeg: bool,
    tick: u64,
    /// This frame's distinct (video layer, lane) requests (reused).
    wanted: Vec<(usize, u8)>,
    /// Where "this editing copy can't be opened" goes (the engine's media sink).
    sink: Option<MediaSink>,
    /// Copies already reported as unusable (each is said once; bounded by the copies the app mapped).
    unusable: std::collections::HashSet<Arc<str>>,
    /// How long to wait to hear whether a file is there (`STAT_LIMIT` for the preview, `EXPORT_STAT_LIMIT` for export:
    /// `patient`), and the preview's redraw signal, fired when a file that wasn't answering answers.
    ask: Ask,
    /// Export's pool (`patient`): a frame stops opening files at the first whose disk didn't answer.
    patient: bool,
    /// Set while a frame waits on a decoder that still makes progress: export's worker sends its heartbeat from it.
    beat: Arc<DecodeBeat>,
}

impl DecoderPool {
    pub fn new(tuning: DecoderTuning, force_ffmpeg: bool) -> Self {
        pressure::watch();
        Self {
            slots: HashMap::new(),
            warming: HashMap::new(),
            tuning,
            force_ffmpeg,
            tick: 0,
            wanted: Vec::new(),
            sink: None,
            unusable: std::collections::HashSet::new(),
            ask: Ask {
                limit: Some(STAT_LIMIT),
                wake: None,
            },
            patient: false,
            beat: Arc::default(),
        }
    }

    /// Set while a frame waits on a decoder that is still making progress (export's heartbeat takes it).
    pub fn decode_beat(&self) -> Arc<DecodeBeat> {
        self.beat.clone()
    }

    /// Waits up to `EXPORT_STAT_LIMIT` to hear whether a file is there (export: a disk that is waking up must never
    /// fail it; one that never answers stops it, named, in `DecodedSet::not_answering`).
    pub fn patient(mut self) -> Self {
        self.ask.limit = Some(EXPORT_STAT_LIMIT);
        self.patient = true;
        self
    }

    /// The preview's redraw signal: fired when a file whose disk didn't answer in time answers (there, missing or
    /// unreadable), so a paused still is drawn again with it.
    pub fn set_redraw(&mut self, redraw: Arc<tokio::sync::Notify>) {
        self.ask.wake = Some(redraw);
    }

    /// Where an editing copy that can't be opened is reported (`MediaEvent::Changed { kind: Proxy }`, once per copy).
    pub fn set_media_sink(&mut self, sink: MediaSink) {
        self.sink = Some(sink);
    }

    /// The main footage gets the pool's tuning; the camera half its cache, an overlay at most 32 MiB; a second lane
    /// (primed for the next cut) half its role's. With an output box (preview), the main footage decodes at most
    /// 1920×1080 and the camera and overlays at most 1280×720: larger sources are scaled while decoding, so frames are
    /// smaller in the cache and on the GPU.
    pub fn tuning_for(&self, role: VideoRole, lane: u8) -> DecoderTuning {
        let main = self.tuning.max_cache_bytes;
        let (max_cache_bytes, max_output) = match role {
            VideoRole::Main => (main, MAIN_BOX),
            VideoRole::Camera => (main / 2, SMALL_BOX),
            VideoRole::Overlay => (main.min(OVERLAY_CACHE_BYTES), SMALL_BOX),
        };
        DecoderTuning {
            max_cache_bytes: max_cache_bytes >> lane.min(1),
            max_output: self.tuning.max_output.map(|_| max_output),
            ..self.tuning
        }
    }

    /// How many decoders are open or remembered as failed.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// How many decoders are still being opened by `warm`.
    pub fn warming(&self) -> usize {
        self.warming.len()
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.cancel_warming();
        self.unusable.clear();
    }

    /// Decodes every distinct (asset, time) the plan's video layers show, concurrently, into `set` (one entry per
    /// video layer, in `FramePlan::videos()` order). A source that can't be opened or decoded is left out and named
    /// in `warnings`, or in `missing` when its file isn't there.
    pub async fn decode(&mut self, plan: &FramePlan, set: &mut DecodedSet) {
        self.decode_unless(plan, set, None).await;
    }

    /// `decode` for a still a newer seek may replace (`superseded`, the engine's seek): in a GOP of at most a second
    /// (an editing copy's, a short-GOP original's), a read toward a frame it replaced stops at the nearest frame of that
    /// GOP read so far, so the next seek's read starts at once instead of after the rest of the GOP (a scrub over
    /// an editing copy at speed). Long-GOP sources are always read to their exact frame.
    pub async fn decode_unless(
        &mut self,
        plan: &FramePlan,
        set: &mut DecodedSet,
        superseded: Option<&Superseded>,
    ) {
        self.tick += 1;
        set.frames.clear();
        set.warnings.clear();
        set.missing.clear();
        set.not_answering.clear();
        // Every video layer, a transition's two sides included (`FramePlan::videos`; no allocation).
        let videos = || plan.videos();
        // Lanes: the one whose reader already sits at (or just before) the time, so a cut to a primed position never
        // reseeks (`lanes`); a second time of one asset in the same frame takes the other lane.
        let mut wanted = std::mem::take(&mut self.wanted);
        wanted.clear();
        for (i, v) in videos().enumerate() {
            let mut taken = [false; NAMES.len()];
            let mut repeat = false;
            for &(j, l) in &wanted {
                let other = &videos().nth(j).expect("indexed above").source;
                if other.asset_id == v.source.asset_id {
                    repeat |= other.time_us == v.source.time_us;
                    taken[usize::from(l)] = true;
                }
            }
            if !repeat {
                let lane =
                    self.lane_for(&v.source.asset_id, &v.source.path, v.source.time_us, &taken);
                wanted.push((i, lane));
            }
        }
        let mut requests = Vec::with_capacity(wanted.len());
        for &(i, lane) in &wanted {
            let v = videos().nth(i).expect("indexed above");
            match self.slot(&v.source, lane, v.role).await {
                Ok(decoder) => {
                    self.place(&v.source.asset_id, lane, v.source.time_us);
                    let superseded = superseded.cloned();
                    let how = Recovery {
                        ask: self.ask.clone(),
                        beat: self.beat.clone(),
                        lane,
                        tuning: self.tuning_for(v.role, lane),
                        force_ffmpeg: self.force_ffmpeg,
                        reopen_quiet: (!self.patient).then(reopen_quiet),
                    };
                    requests.push(async move {
                        // A failed frame says what its decoder's file is now: gone (moved or renamed, its drive
                        // removed), there, unreadable, or not answering, asked off the engine's task. Only a file
                        // that is there is read again: by a reader opened afresh if this one stalled, then in
                        // software.
                        let Read { frame, reopened } =
                            decoder.read(v.source.time_us, superseded, &how).await;
                        (i, lane, frame, reopened)
                    })
                }
                Err(failure) => {
                    set.report(failure, &v.source.original);
                    // Export stops at a disk that didn't answer within its minute (the frame fails, naming it):
                    // opening the frame's other files, or reading the ones already open, would only add their waits,
                    // up to the parent's watchdog. Requests not yet polled have sent nothing.
                    if failure == Failure::NotAnswering && self.patient {
                        requests.clear();
                        break;
                    }
                }
            }
        }
        let mut decoded = futures::future::join_all(requests).await;
        for (i, lane, frame, reopened) in &mut decoded {
            let key = || {
                let asset = &videos().nth(*i).expect("indexed above").source.asset_id;
                (asset.clone(), *lane)
            };
            // Its reader stalled and was opened again: the new one serves the slot from now on, and the stuck one goes
            // when the last request holding it does (its thread ends once its call returns; never cancelled from here).
            if let Some(new) = reopened.take()
                && let Some(slot) = self.slots.get_mut(&key())
            {
                slot.state = State::Ready(new);
            }
            match frame {
                // Its file went away while it was open: it is let go, so the next frame opens the source again (an
                // editing copy that went falls back to its original; an original that went is missing at open and
                // tried again only after `RETRY_AFTER`), never reading it through a fallback.
                Err(FileState::Missing) => {
                    self.slots.remove(&key());
                }
                // Its disk didn't answer: kept, and the next frame asks that stat before the reader.
                Err(FileState::NotAnswering) => {
                    if let Some(slot) = self.slots.get_mut(&key()) {
                        slot.silent = true;
                    }
                }
                _ => {}
            }
        }
        for v in videos() {
            let found = decoded.iter().find(|(i, _, _, _)| {
                let s = &videos().nth(*i).expect("indexed above").source;
                s.asset_id == v.source.asset_id && s.time_us == v.source.time_us
            });
            match found {
                Some((_, _, Ok(frame), _)) => set.frames.push(frame.clone()),
                Some((_, _, Err(state), _)) => {
                    // A disk that hasn't answered isn't said (the decoder stays, and the redraw signal fires when it
                    // answers); a copy that went says nothing (the next frame reads the original); a source that
                    // went is missing; one still there couldn't be read.
                    match state {
                        FileState::NotAnswering => {
                            set.report(Failure::NotAnswering, &v.source.original)
                        }
                        FileState::Missing => {
                            match file_state(&v.source.original, &self.ask).await {
                                FileState::Missing => {
                                    set.report(Failure::Missing, &v.source.original)
                                }
                                FileState::Unreadable => {
                                    set.report(Failure::Unreadable, &v.source.original)
                                }
                                FileState::NotAnswering => {
                                    set.report(Failure::NotAnswering, &v.source.original)
                                }
                                FileState::There => {}
                            }
                        }
                        FileState::There | FileState::Unreadable => {
                            set.report(Failure::Unreadable, &v.source.original)
                        }
                    }
                    set.frames.push(None);
                }
                None => set.frames.push(None),
            }
        }
        set.warnings.dedup();
        set.missing.dedup();
        set.not_answering.dedup();
        self.wanted = wanted;
        self.evict();
    }
}
