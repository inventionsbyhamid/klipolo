//! The start screen's posters: every listed project's card made sharp without opening the project. Listing the
//! projects (and a listing of its own soon after launch) asks for the posters their cards still lack (`poster_wants`),
//! one per file; this makes them one file at a time on a thread of its own at utility QoS, through the engine's poster
//! maker (`klipolo_engine::media::poster`), and tells each card's project as its poster lands (`ProjectPoster`), so the
//! card swaps to it.
//!
//! Low priority: each poster waits while the video plays (and for a moment after, as other picture work does), while
//! an export runs and while a take records: analysis's holds (`Holds`, `Work::Picture`), which the app keeps told. A
//! new listing replaces what was still waiting. A file whose poster can't be made (no picture, unreadable) isn't tried
//! again in this run.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use klipolo_app::analysis::holds::{Holds, Work};
use klipolo_engine::media::thumbs::{Poster, poster};
use serde::Serialize;
use ts_rs::TS;

use super::recent::{PosterWant, ThumbTile};

/// Makes `file`'s poster in the thumbnails folder: the engine's maker, or a stand-in in tests.
pub(crate) type Make = dyn Fn(&Path, &str) -> Result<Option<Poster>, String> + Send + Sync;
/// Hears each card's poster as it lands.
pub(crate) type Tell = dyn Fn(ProjectPoster) + Send + Sync;

/// A card's poster, made: the project's file and its card's new picture.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectPoster {
    pub path: String,
    pub thumb: ThumbTile,
}

/// The posters still to make, and the thread that makes them (one, while there are any).
pub struct Posters {
    inner: Arc<Inner>,
}

struct Inner {
    /// The engine's thumbnails folder.
    root: PathBuf,
    holds: Arc<Holds>,
    make: Arc<Make>,
    tell: Arc<Tell>,
    queue: Mutex<Queue>,
}

#[derive(Default)]
struct Queue {
    wants: VecDeque<PosterWant>,
    /// The file being made (or waiting for the holds to go).
    current: Option<String>,
    worker: bool,
    /// Files whose poster couldn't be made in this run.
    given_up: HashSet<String>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Posters {
    pub(crate) fn new(root: PathBuf, holds: Arc<Holds>, make: Arc<Make>, tell: Arc<Tell>) -> Self {
        Posters {
            inner: Arc::new(Inner {
                root,
                holds,
                make,
                tell,
                queue: Mutex::default(),
            }),
        }
    }

    /// The posters a listing's cards wait for, in its order: replaces what was still waiting. A file being made now, or
    /// given up on in this run, is left out.
    pub(crate) fn want(&self, wants: Vec<PosterWant>) {
        let mut q = lock(&self.inner.queue);
        let skip: HashSet<String> = q.given_up.iter().chain(&q.current).cloned().collect();
        q.wants = wants
            .into_iter()
            .filter(|want| !skip.contains(&want.file))
            .collect();
        if q.wants.is_empty() || q.worker {
            return;
        }
        q.worker = true;
        let inner = self.inner.clone();
        let spawned = std::thread::Builder::new()
            .name("klipolo-posters".into())
            .spawn(move || inner.work());
        if let Err(error) = spawned {
            q.worker = false;
            q.wants.clear();
            tracing::warn!(%error, "the posters' thread");
        }
    }

    /// Posters are waiting or being made.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> bool {
        let q = lock(&self.inner.queue);
        !q.wants.is_empty() || q.current.is_some()
    }
}

impl Inner {
    /// Makes the posters asked for, one at a time, then ends.
    fn work(self: Arc<Self>) {
        klipolo_platform::qos::set_current(klipolo_platform::qos::Qos::Utility);
        // Nothing stops a poster but the app ending: a hold only makes it wait.
        let never = AtomicBool::new(false);
        loop {
            let want = {
                let mut q = lock(&self.queue);
                let Some(want) = q.wants.pop_front() else {
                    q.worker = false;
                    return;
                };
                q.current = Some(want.file.clone());
                want
            };
            self.holds.wait(Work::Picture, &never);
            let made = match poster(&self.root, &want.file).filter(|p| p.current) {
                // Made meanwhile (an earlier listing's poster of the same file).
                Some(made) => Ok(Some(made)),
                None => (self.make)(&self.root, &want.file),
            };
            let mut given_up = false;
            match made {
                Ok(Some(made)) => {
                    for (path, crop) in want.cards {
                        (self.tell)(ProjectPoster {
                            path,
                            thumb: ThumbTile {
                                sprite: made.path.clone(),
                                tile_w: made.w,
                                tile_h: made.h,
                                cols: 1,
                                crop,
                            },
                        });
                    }
                }
                Ok(None) => given_up = true,
                Err(error) => {
                    tracing::warn!(%error, file = %file_name(&want.file), "a start screen poster wasn't made");
                    given_up = true;
                }
            }
            let mut q = lock(&self.queue);
            q.current = None;
            if given_up {
                q.given_up.insert(want.file);
            }
        }
    }
}

/// A file named by its name only in the log.
fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
#[path = "posters_tests.rs"]
mod tests;
