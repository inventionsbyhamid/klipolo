//! A take's cursor in the engine: its track, its facts (with its screen activity, `follow`), which of its
//! shape images are usable, one smoothed path per smoothing its clips draw with, and its follow-crop paths (`follow`).
//!
//! **Built off the frame path.** `TakeCursor::new` (the track's facts and the shape files' checks) and `prepare` (the
//! smoothed paths) run on the loader thread, or export's blocking load before its first frame: never the render thread
//! or the actor. A path is built outside the lock, so `smoothed` never waits on a build.
//!
//! **One source of truth.** The facts are `TakeFacts::of_cursor`'s (line 171): the dwells live only
//! there. The track is shared (`Arc`), so `timeline::cursor_at` reads it with no copy. The still runs
//! are `StillRuns::of`'s, which ends the last one at a burn-in (it also keeps typing from
//! running past it).
//!
//! **Memory** (`bytes`): the track (8 B a position), the facts (a few KB), the shared rest scan (16 B a rest, held only
//! while a path is) and each variant (4 B a step of motion, ≤ 1.73 MB an hour). The cache's budget counts
//! it all, the same in preview and export.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use klipolo_cursor::facts::{Dwell, StillRuns};
use klipolo_cursor::follow::FollowPath;
use klipolo_cursor::smooth::{Rests, SmoothedPath};
use klipolo_cursor::timeline::TakeFacts;
use klipolo_cursor::{CursorTrack, Press, RegionSpan, Us};
use klipolo_model::cursor_style::Smoothing;

use super::follow::FollowKey;
use super::shapes;
use super::variants::Variants;
use super::want::Want;

/// A take's cursor (see the module notes).
pub struct TakeCursor {
    pub track: Arc<CursorTrack>,
    pub still: StillRuns,
    /// Per shape, in `track.shapes()`'s order (by id): its PNG's checked path, when it is inside the take's `cursors/`,
    /// exists, and its header says ≤ 256 × 256; None draws the Arrow look. `shape_file` looks one up by
    /// id; the cursor layer hands the path on with no copy.
    pub shape_files: Vec<Option<Arc<str>>>,
    /// The shapes whose image turned out not to decode (bit i: `track.shapes()[i]`, at most `MAX_SHAPES` = 64), set by
    /// export's preload and the preview's decoder alike (`refuse_shape`): both then plan the Arrow look there. An
    /// concern 6). An atomic, so a frame reads it with no lock and no allocation.
    refused: AtomicU64,
    /// The take's folder (shape paths are joined to it, then checked to stay inside it).
    pub dir: PathBuf,
    /// What the cursor file alone gives.
    pub(super) base: Arc<TakeFacts>,
    /// The facts with the screen activity read last, and that activity's fingerprint (0: none read yet).
    pub(super) known: Mutex<(u64, Arc<TakeFacts>)>,
    /// One path per crop its clips follow with (`follow`), at most `want::MAX_FOLLOWS`.
    pub(super) follows: Mutex<Variants<FollowKey, Arc<FollowPath>>>,
    /// Found with the first smoothed path, shared by the take's others, and let go with the last
    /// path counts what a freshly loaded one does).
    rests: Mutex<Option<Arc<Rests>>>,
    variants: Mutex<Variants<Smoothing, Arc<SmoothedPath>>>,
}

pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The smoothings that build a path: Off draws the raw path.
fn built(smoothings: &[Smoothing]) -> impl Iterator<Item = Smoothing> + '_ {
    smoothings.iter().copied().filter(|&s| s != Smoothing::Off)
}

impl TakeCursor {
    /// Built by `load_take` on the loader thread: O(samples + shapes).
    pub fn new(track: CursorTrack, dir: PathBuf) -> Self {
        let still = StillRuns::of(&track);
        let base = Arc::new(TakeFacts::of_cursor(&track, &still));
        let shape_files = shapes::usable(&dir, track.shapes());
        Self {
            track: Arc::new(track),
            still,
            shape_files,
            refused: AtomicU64::new(0),
            dir,
            known: Mutex::new((0, base.clone())),
            base,
            follows: Mutex::default(),
            rests: Mutex::default(),
            variants: Mutex::default(),
        }
    }

    /// The smoothed path for `s` once built (None until then, and always for Off). O(1). A frame reads the path it
    /// draws with through `with_drawn`, which holds no reference past its call.
    pub fn smoothed(&self, s: Smoothing) -> Option<Arc<SmoothedPath>> {
        lock(&self.variants).get(s)
    }

    /// `f` on the path a clip drawn with `s` draws now, under the take's lock, so a frame never holds the last
    /// reference to a path the loader lets go and never frees one. That is `s`'s own path once built;
    /// while it builds, the held path nearest to it (the one the clip drew before is there until `keep`,
    /// so a style change never shows the raw path in between); the raw path (an empty `SmoothedPath`, which samples
    /// as the track) for Off or when no path is held. Preview replans when the path lands; export builds every path
    /// before its first frame, so it always draws `s`'s own. O(1), no allocation; `f` mustn't call back into the take.
    pub fn with_drawn<R>(&self, s: Smoothing, f: impl FnOnce(&SmoothedPath) -> R) -> R {
        let raw = SmoothedPath::default();
        if s == Smoothing::Off {
            return f(&raw);
        }
        let mut variants = lock(&self.variants);
        if let Some(path) = variants.get_ref(s) {
            return f(path);
        }
        let from = s as u32;
        let rank =
            |k: Smoothing| (k != Smoothing::Off).then(|| from.abs_diff(k as u32) * 4 + k as u32);
        match variants.best_by(rank) {
            Some(path) => f(path),
            None => f(&raw),
        }
    }

    /// Builds what's missing (the loader thread, or export's blocking load), then lets go of a path no clip draws with
    /// now, and of the rest scan with the last path. What goes is freed after the lock. O(steps) a path
    /// built (≤ 30 ms an hour of motion in release), O(1) when nothing is missing.
    pub fn prepare(&self, smoothings: &[Smoothing]) {
        let wanted: Vec<Smoothing> = built(smoothings).collect();
        for &s in &wanted {
            if lock(&self.variants).contains(s) {
                continue;
            }
            let rests = self.rests();
            let path = Arc::new(SmoothedPath::build_with(&self.track, s, &rests));
            let gone = lock(&self.variants).put(s, path);
            drop(gone);
        }
        let gone = lock(&self.variants).keep(&wanted);
        drop(gone);
        if wanted.is_empty() {
            let gone = lock(&self.rests).take();
            drop(gone);
        }
    }

    /// The take's rest scan, found now if no path holds it (outside the lock: only the loader prepares a take).
    fn rests(&self) -> Arc<Rests> {
        if let Some(rests) = lock(&self.rests).clone() {
            return rests;
        }
        let rests = Arc::new(Rests::of(&self.track));
        lock(&self.rests).get_or_insert(rests).clone()
    }

    /// Holds a path for each of `smoothings` (Off needs none) and no other: what `prepare` leaves.
    pub fn ready(&self, smoothings: &[Smoothing]) -> bool {
        let wanted: Vec<Smoothing> = built(smoothings).collect();
        lock(&self.variants).holds_exactly(&wanted)
    }

    /// Holds what `want` asks for with activity `print` (`follow::Screens`): its smoothed paths, the facts of that
    /// activity, and its follow paths.
    pub fn ready_for(&self, want: &Want, print: u64) -> bool {
        self.ready(&want.smoothings) && self.follows_ready(&want.crops, print)
    }

    /// The smoothed paths held.
    pub fn variants(&self) -> usize {
        lock(&self.variants).len()
    }

    /// This take's facts for `klipolo_cursor::timeline::on_timeline`: the cursor file's, with the screen activity read
    /// last (typing, playing media and quiet, on the recording clock; `prepare_follow`).
    pub fn facts(&self) -> Arc<TakeFacts> {
        lock(&self.known).1.clone()
    }

    /// Where the cursor paused after travelling: the facts' own.
    pub fn dwells(&self) -> &[Dwell] {
        &self.base.dwells
    }

    /// Shape `id`'s image is usable (a shape the file doesn't define isn't). O(log shapes).
    pub fn shape_usable(&self, id: u16) -> bool {
        self.shape_file(id).is_some()
    }

    /// Shape `id`'s checked image path, if it is usable (and its image wasn't refused since). O(log shapes), no
    /// allocation.
    pub fn shape_file(&self, id: u16) -> Option<&Arc<str>> {
        let i = self.shape_index(id)?;
        if self.refused.load(Ordering::Relaxed) & bit(i) != 0 {
            return None;
        }
        self.shape_files.get(i)?.as_ref()
    }

    /// Shape `id`'s image can't be used after all (it didn't decode): from now on the take plans the Arrow look for
    /// it. True if it was usable until now. O(log shapes), no allocation.
    pub fn refuse_shape(&self, id: u16) -> bool {
        let Some(i) = self.shape_index(id) else {
            return false;
        };
        self.shape_files.get(i).is_some_and(Option::is_some)
            && self.refused.fetch_or(bit(i), Ordering::Relaxed) & bit(i) == 0
    }

    fn shape_index(&self, id: u16) -> Option<usize> {
        self.track.shapes().binary_search_by_key(&id, |s| s.id).ok()
    }

    /// What the take holds: its track, facts, rests and paths (smoothed and followed).
    pub fn bytes(&self) -> usize {
        let paths: usize = lock(&self.variants).values().map(|p| p.bytes()).sum();
        let known = lock(&self.known).1.clone();
        let activity = if Arc::ptr_eq(&known, &self.base) {
            0
        } else {
            facts_bytes(&known)
        };
        size_of::<Self>()
            + self.track.bytes()
            + size_of_val(self.still.runs())
            + facts_bytes(&self.base)
            + activity
            + self.follow_bytes()
            + self.shape_files.capacity() * size_of::<Option<Arc<str>>>()
            + self
                .shape_files
                .iter()
                .flatten()
                .map(|f| f.len())
                .sum::<usize>()
            + self.dir.as_os_str().len()
            + lock(&self.rests).as_ref().map_or(0, |r| r.bytes())
            + paths
    }
}

/// Shape `i`'s bit in `TakeCursor::refused` (none past 64: the reader keeps at most `MAX_SHAPES`).
fn bit(i: usize) -> u64 {
    u32::try_from(i)
        .ok()
        .and_then(|i| 1u64.checked_shl(i))
        .unwrap_or(0)
}

/// Every list a take's facts hold, the activity's too.
fn facts_bytes(f: &TakeFacts) -> usize {
    let spans = |s: &Option<Vec<(Us, Us)>>| {
        s.as_ref()
            .map_or(0, |s| s.capacity() * size_of::<(Us, Us)>())
    };
    let regions = |r: &Option<Vec<RegionSpan>>| {
        r.as_ref()
            .map_or(0, |r| r.capacity() * size_of::<RegionSpan>())
    };
    size_of::<TakeFacts>()
        + f.presses.capacity() * size_of::<Press>()
        + f.dwells.capacity() * size_of::<Dwell>()
        + f.still.capacity() * size_of::<(Us, Us)>()
        + regions(&f.typing)
        + regions(&f.media)
        + spans(&f.quiet_screen)
        + spans(&f.quiet_sound)
}
