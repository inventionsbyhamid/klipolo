//! The engine's cursor takes by file, under one byte budget (over takes), loaded and
//! smoothed on their own thread.
//!
//! A project names the takes it needs in timeline order, with the smoothings their clips draw with (`wanted`). The
//! plan follows the longest run of that list whose loaded takes fit `BUDGET_BYTES` (`fitting`): a take not loaded yet,
//! or whose file can't be read, is passed over and takes none of it; the first loaded take that doesn't fit ends the
//! run. The loader reads the list in the same order, builds each take's smoothed paths, and stops once the takes it
//! holds pass the budget; export does the same before its first frame (`load_all`), so preview and export hold the same
//! takes with the same paths. Real projects rarely come near it: an hour of a tutorial take is about 3 MB with its
//! path, an hour of continuous 250 Hz motion about 9 MB, and the budget holds four of the largest.
//!
//! Memory: the wanted takes stay within the budget plus the one take that passed it (≤ `CursorTrack::MAX_BYTES` and
//! four paths), a hard cap: takes no longer wanted go first, least recently used first, whenever the
//! total passes the budget, then any wanted take past that one (export would never load it).
//!
//! The loader thread is `queue`'s.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use klipolo_cursor::CursorTrack;
use tokio::sync::Notify;

use super::follow::{ActivityOf, Screens};
use super::queue;
use super::state::{Gone, State, lock};
use super::take::TakeCursor;
use super::want::Want;

/// What the engine's cursor takes may hold together.
pub const BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// The engine's cursor takes by file (see the module notes).
pub struct CursorTracks {
    pub(super) shared: Arc<Shared>,
}

impl Default for CursorTracks {
    fn default() -> Self {
        Self::with_budget(BUDGET_BYTES)
    }
}

pub(super) struct Shared {
    pub(super) state: Mutex<State>,
    /// Signalled after each background job (a take loaded or smoothed, or a file that couldn't be read).
    pub(super) loaded: Arc<Notify>,
    /// The same, for `wait_ready`.
    pub(super) ready: Condvar,
    pub(super) budget: usize,
    /// Where the takes' screen activity comes from (the session's store); none: their facts and follow paths go
    /// without it.
    pub(super) activity: Mutex<Option<ActivityOf>>,
    /// Frames' reads of a take (`with_take`), for the test that a burned-in take never reads one.
    reads: AtomicU64,
}

impl Shared {
    /// `w`'s screen files' activity as the store holds it now.
    pub(super) fn screens(&self, w: &Want) -> Screens {
        Screens::read(w.activity_screens(), lock(&self.activity).as_ref())
    }
}

impl CursorTracks {
    /// A cache whose takes share `budget` bytes (tests make it small).
    pub fn with_budget(budget: usize) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::default(),
                loaded: Arc::default(),
                ready: Condvar::new(),
                budget,
                activity: Mutex::default(),
                reads: AtomicU64::new(0),
            }),
        }
    }

    /// The takes the plan follows for `wants` (a project's `wanted`, in order): each loaded one while they fit the
    /// budget, passing over takes not loaded or unreadable, until the first loaded take that doesn't fit. They become
    /// the most recently used. O(wants × takes).
    pub fn fitting(&self, wants: &[Want]) -> Vec<(String, Arc<TakeCursor>)> {
        self.fitting_paths(wants.iter().map(|w| w.path.as_str()))
    }

    /// `fitting` by the takes' cursor files alone, in `wanted`'s order (`want::take_paths`): what a plan context reads on
    /// every edit, with no crops worked out. O(paths × takes).
    pub fn fitting_paths<'a>(
        &self,
        paths: impl IntoIterator<Item = &'a str>,
    ) -> Vec<(String, Arc<TakeCursor>)> {
        let mut s = lock(&self.shared.state);
        let mut held = 0;
        let mut out = Vec::new();
        for path in paths {
            let Some(take) = s.take(path).cloned() else {
                continue;
            };
            held += take.bytes();
            if held > self.shared.budget {
                break;
            }
            out.push((path.to_owned(), take));
        }
        for (path, _) in &out {
            if let Some(i) = s.takes.iter().position(|(p, _)| **p == **path) {
                let entry = s.takes.remove(i);
                s.takes.push(entry);
            }
        }
        out
    }

    /// `path`'s take if it is loaded.
    pub fn get(&self, path: &str) -> Option<Arc<TakeCursor>> {
        lock(&self.shared.state).take(path).cloned()
    }

    /// `f` on `path`'s take and its key (the cache's own shared path), if it is loaded, under the cache's lock: a frame
    /// never holds the last reference to a take the loader lets go, so it never frees one. O(takes), no
    /// allocation; `f` mustn't call back into the cache.
    pub fn with_take<R>(
        &self,
        path: &str,
        f: impl FnOnce(&Arc<str>, &TakeCursor) -> R,
    ) -> Option<R> {
        self.shared.reads.fetch_add(1, Ordering::Relaxed);
        let s = lock(&self.shared.state);
        let (key, take) = s
            .takes
            .iter()
            .find(|(p, _)| **p == *path)
            .and_then(|(p, t)| Some((p, t.as_ref()?)))?;
        Some(f(key, take))
    }

    /// How many times frames read a take (`with_take`): the cursor layer's test that a burned-in take never does.
    #[doc(hidden)]
    pub fn reads(&self) -> u64 {
        self.shared.reads.load(Ordering::Relaxed)
    }

    /// The takes' screen activity comes from `of` from now on (the session's activity store); the next `want` builds
    /// their facts and follow paths again for what it holds.
    pub fn set_activity(&self, of: ActivityOf) {
        *lock(&self.shared.activity) = Some(of);
    }

    /// The session, on each publish and each time the screen activity grows: the project's takes in timeline order.
    /// Those not loaded yet (and those whose file couldn't be read before) load in the background, in that order, while
    /// the takes held stay within the budget; a loaded take whose clips now draw with other smoothings or crops, or
    /// whose activity grew, is built again. `wake` fires after each. O(wanted × takes) (the
    /// budget's cut is found once).
    pub fn want(&self, wants: &[Want]) {
        let prints: Vec<u64> = wants.iter().map(|w| self.shared.screens(w).print).collect();
        let mut s = lock(&self.shared.state);
        s.wanted = wants.to_vec();
        let gone = s.trim(self.shared.budget);
        let cut = s.worth_until(self.shared.budget);
        for (w, &print) in wants.iter().zip(&prints).take(cut) {
            if s.needs(w, print) {
                s.queue.push_back(w.path.as_str().into());
            }
        }
        queue::start(&self.shared, &mut s);
        drop(s);
        gone.free_elsewhere();
    }

    /// Export, before its first frame: `wants` loaded and smoothed now on the caller's thread, in order, while the takes
    /// held stay within the budget, as the background loader does them.
    pub fn load_all(&self, wants: &[Want]) {
        lock(&self.shared.state).wanted = wants.to_vec();
        for w in wants {
            let existing = {
                let s = lock(&self.shared.state);
                if !s.worth_loading(&w.path, self.shared.budget) {
                    break;
                }
                s.take(&w.path).cloned()
            };
            let take = queue::job(&w.path, existing, w, &self.shared);
            // Freed here, on export's loading thread, after the lock.
            let gone =
                lock(&self.shared.state).put(w.path.as_str().into(), take, self.shared.budget);
            drop(gone);
        }
        self.shared.ready.notify_all();
    }

    /// Test and export helper: blocks until every wanted take the budget holds has exactly its paths (or its file
    /// couldn't be read), at most `timeout`. False when it times out, or when nothing more will load (`want` wasn't
    /// called with these).
    pub fn wait_ready(&self, wants: &[Want], timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let prints: Vec<u64> = wants.iter().map(|w| self.shared.screens(w).print).collect();
        let mut s = lock(&self.shared.state);
        loop {
            if s.settled(wants, &prints, self.shared.budget) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline || (!s.worker && s.queue.is_empty()) {
                return false;
            }
            s = (self.shared.ready)
                .wait_timeout(s, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Puts a track read elsewhere in the cache as a take, with no smoothed path yet (tests).
    pub fn insert(&self, path: &str, track: CursorTrack) {
        let dir = Path::new(path)
            .parent()
            .filter(|d| !d.as_os_str().is_empty());
        let dir = dir.unwrap_or(Path::new(".")).to_path_buf();
        let take = Arc::new(TakeCursor::new(track, dir));
        let gone = lock(&self.shared.state).put(path.into(), Some(take), self.shared.budget);
        drop(gone);
    }

    /// The project closed or switched (on the actor): every take and queued job goes, and a job under way is dropped
    /// when it ends (the same file asked for again by the next project loads again). The takes are freed on a thread
    /// of their own (up to the whole budget and more).
    pub fn clear(&self) {
        let mut s = lock(&self.shared.state);
        let gone = Gone(s.takes.drain(..).filter_map(|(_, t)| t).collect());
        s.wanted.clear();
        s.queue.clear();
        s.loading = None;
        s.generation = s.generation.wrapping_add(1);
        self.shared.ready.notify_all();
        drop(s);
        gone.free_elsewhere();
    }

    /// Signalled after each background job; one that ends while nobody waits is kept for the next wait.
    pub fn wake(&self) -> Arc<Notify> {
        self.shared.loaded.clone()
    }
}
