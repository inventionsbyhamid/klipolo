//! `ActivityStore`: the screen files a project shows, analysed one at a time on a utility-QoS thread after the sound
//! cache and the thumbnails (`ThumbStore`'s holds, shared), and their tracks kept in memory by least recent use.
//!
//! **Waiting.** A file starts only when nothing holds background work (playback, an export, an editing copy, a take
//! recording, the sound cache decoding) and thumbnails have nothing to do. A hold that starts mid-pass stops decoding
//! at once. An export, a take or an editing copy ends the pass at once; a hold that outlasts its grace does too
//! (`ThumbStore::wait_quiet_keeping`: up to 10 s for playback, a second for the order's own holds). Every chunk made
//! is kept, the decoder and the file are let go, and the file is first in line again once nothing holds it. A short
//! hold (a tap of Play, an edit's thumbnail check) goes on where it was, with the same decoder.
//!
//! **Failures** never block a take: a file whose reading fails part-way is tried `READ_TRIES` times in an opening (a
//! minute it can't read is passed over, the rest made: `run`), one that can't be analysed is marked so in the folder
//! and planned without (`failed`), a full disk stops the pass until the next opening, and a file another process is
//! writing is tried again after `BUSY_AGAIN`. A missing or damaged activity file only means less is known.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use klipolo_cursor::activity::ActivityTrack;
use tokio::sync::Notify;

use super::files::files;
use super::lru::Lru;
use super::run::{Pace, Pass};
use crate::media::events::{MediaEvent, MediaKind, MediaSink};
use crate::media::key::{CacheKind, stem};
use crate::media::store::ThumbStore;
use crate::platform::activity::{Opener, platform_opener};

/// The tracks held in memory: about 16 hours of screen video.
pub const MEMORY_BYTES: usize = 8 << 20;
/// The thread ends after this long with nothing to do.
const IDLE: Duration = Duration::from_secs(30);
/// A file another process is writing is looked at again after this long.
const BUSY_AGAIN: Duration = Duration::from_secs(2);
/// How long `ensure` waits for another pass to let a file go (the editor's lets go at once for an export; export tries
/// again itself, `RETRY_WAITS`).
const ENSURE_WAIT: Duration = Duration::from_secs(2);

#[derive(Default)]
struct State {
    queue: VecDeque<String>,
    worker: bool,
    /// The file being analysed and its stop flag.
    current: Option<(String, Arc<AtomicBool>)>,
    tracks: Lru,
    /// Files that can't be analysed, or whose reading kept failing, in this opening.
    gave_up: HashSet<String>,
    unreadable: HashMap<String, u32>,
    disk_full: bool,
}

pub struct ActivityStore {
    root: PathBuf,
    thumbs: Weak<ThumbStore>,
    opener: Opener,
    state: Mutex<State>,
    /// Signalled when files are wanted.
    changed: Condvar,
    wake: Arc<Notify>,
    sink: Mutex<Option<MediaSink>>,
}

mod ensure;
mod failed;
mod worker;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ActivityStore {
    /// The store after `thumbs` in line, its files in `root` (the media cache's `activity` folder); `thumbs` holds it.
    pub fn attach(thumbs: &Arc<ThumbStore>, root: PathBuf) -> Arc<ActivityStore> {
        Self::attach_with(thumbs, root, platform_opener())
    }

    /// `attach` with another decoder (tests: a synthetic screen).
    #[doc(hidden)]
    pub fn attach_with(
        thumbs: &Arc<ThumbStore>,
        root: PathBuf,
        opener: Opener,
    ) -> Arc<ActivityStore> {
        let store = Self::with_thumbs(root, Arc::downgrade(thumbs), opener);
        thumbs.set_next(store.clone());
        store
    }

    /// A store of its own on `root` with no background pass and no thumbnails before it (an export, another
    /// process): `ensure` reads and makes files in the folder it shares with the editor's store.
    pub fn detached(root: PathBuf, opener: Opener) -> Arc<ActivityStore> {
        Self::with_thumbs(root, Weak::new(), opener)
    }

    fn with_thumbs(root: PathBuf, thumbs: Weak<ThumbStore>, opener: Opener) -> Arc<ActivityStore> {
        Arc::new(ActivityStore {
            root,
            thumbs,
            opener,
            state: Mutex::default(),
            changed: Condvar::new(),
            wake: Arc::default(),
            sink: Mutex::default(),
        })
    }

    /// `path`'s activity is all made, in memory or in the folder: `ensure` reads it, decoding nothing. O(tracks), and a
    /// stat or two.
    pub fn is_analysed(&self, path: &str) -> bool {
        if lock(&self.state)
            .tracks
            .peek(path)
            .is_some_and(|t| t.is_complete())
        {
            return true;
        }
        stem(path, &CacheKind::Activity).is_some_and(|stem| files(&self.root, &stem).0.exists())
    }

    pub(crate) fn set_sink(&self, sink: MediaSink) {
        *lock(&self.sink) = Some(sink);
    }

    /// `path`'s track if it is in memory (partial while its pass runs: `complete_through`). O(tracks), no IO.
    pub fn get(&self, path: &str) -> Option<Arc<ActivityTrack>> {
        lock(&self.state).tracks.get(path)
    }

    /// The project's screen files in the order to analyse them: replaces the queue. Files analysed already are read
    /// from the cache in the background; the rest are analysed. Cheap enough for every edit: no IO here.
    pub fn want(self: &Arc<Self>, paths: &[String]) {
        let mut s = lock(&self.state);
        let current = s.current.as_ref().map(|(p, _)| p.clone());
        let queue: VecDeque<String> = paths
            .iter()
            .filter(|p| Some(*p) != current.as_ref() && !s.gave_up.contains(*p))
            .filter(|p| !s.tracks.peek(p).is_some_and(|t| t.is_complete()))
            .cloned()
            .collect();
        s.queue = queue;
        if !s.queue.is_empty() && !s.worker {
            s.worker = true;
            let store = self.clone();
            let spawned = std::thread::Builder::new()
                .name("klipolo-activity".into())
                .spawn(move || store.work());
            if let Err(error) = spawned {
                s.worker = false;
                tracing::warn!(%error, "activity thread");
            }
        }
        drop(s);
        self.changed.notify_all();
    }

    /// Signalled whenever a track grows or is read from the cache; one that comes while nobody waits is kept.
    pub fn wake(&self) -> Arc<Notify> {
        self.wake.clone()
    }

    /// Files are queued or one is being looked at.
    pub fn pending(&self) -> bool {
        let s = lock(&self.state);
        !s.queue.is_empty() || s.current.is_some()
    }

    /// The project closed or switched (with the thumbnails' `cancel_all`): the queue empties, a running pass stops
    /// within a tick keeping every chunk it made, and the tracks are let go (off the lock).
    pub fn cancel_all(&self) {
        let gone = {
            let mut s = lock(&self.state);
            s.queue.clear();
            s.gave_up.clear();
            s.unreadable.clear();
            s.disk_full = false;
            if let Some((_, cancel)) = &s.current {
                cancel.store(true, Ordering::SeqCst);
            }
            s.tracks.clear()
        };
        if let Some(thumbs) = self.thumbs.upgrade() {
            thumbs.wake_waiters();
        }
        drop(gone);
    }

    /// The background pass over `path`, if any, stops and is first in line again.
    fn preempt(&self, path: &str) {
        let mut s = lock(&self.state);
        if let Some((current, cancel)) = &s.current
            && current == path
        {
            cancel.store(true, Ordering::SeqCst);
            let path = path.to_owned();
            s.queue.retain(|p| *p != path);
            s.queue.push_front(path);
        }
        drop(s);
        if let Some(thumbs) = self.thumbs.upgrade() {
            thumbs.wake_waiters();
        }
    }

    fn pass<'a>(
        &'a self,
        path: &'a str,
        stem: &'a str,
        cancel: &'a Arc<AtomicBool>,
        pace: &'a dyn Fn() -> Pace,
        made: &'a dyn Fn(Arc<ActivityTrack>, f32),
    ) -> Pass<'a> {
        Pass {
            root: &self.root,
            path,
            stem,
            opener: &self.opener,
            cancel,
            pace,
            made,
        }
    }

    /// `track` is `path`'s now: in memory (what it pushes out is freed off the lock), and said.
    fn publish(&self, path: &str, track: Arc<ActivityTrack>) {
        let gone = lock(&self.state).tracks.put(path, track, MEMORY_BYTES);
        drop(gone);
        self.said(path);
    }

    /// `path`'s track changed (or went): the media sink hears it, and the plans read it again (`wake`).
    fn said(&self, path: &str) {
        let sink = lock(&self.sink).clone();
        if let Some(sink) = sink {
            sink(MediaEvent::Changed {
                path: path.to_owned(),
                kind: MediaKind::Activity,
            });
        }
        self.wake.notify_one();
    }
}
