//! The cursor loader: one utility-QoS thread that loads the wanted takes in order and builds the
//! smoothed paths their clips draw with, while the takes held stay within the budget, and wakes the session after each.
//! It runs only while something is queued, never on the render thread or the actor, and never holds the cache's lock
//! while it reads or builds.
//!
//! A job is a take's path: `load_take` then `prepare` and `prepare_follow` when it isn't loaded, else those alone. It
//! reads what the take wants from the latest `want`, and its screen activity from the store, as it starts, and a `want`
//! during a job queues the take again, so the newest smoothings, crops and activity are always built. A job that panics (a bug, never input: the reader refuses what it can't hold) is caught
//! and counted as a file that couldn't be read, so the loader goes on.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;

use super::load_take;
use super::state::{State, lock};
use super::take::TakeCursor;
use super::tracks::Shared;
use super::want::Want;

impl State {
    /// `want`'s take (one of the wanted prefix worth loading, `worth_until`) should be queued: not queued already, and
    /// not loaded with exactly what it wants for activity `print`. A file that couldn't be read is forgotten here, so
    /// it is tried again.
    pub(super) fn needs(&mut self, want: &Want, print: u64) -> bool {
        let path = want.path.as_str();
        self.takes.retain(|(p, t)| t.is_some() || &**p != path);
        if self.queue.iter().any(|p| &**p == path) {
            return false;
        }
        self.take(path)
            .is_none_or(|take| !take.ready_for(want, print))
    }

    /// How many of the wanted takes are worth loading (a take past the budget would only start the
    /// loader to be passed over, on every publish): those before the wanted takes held pass the budget. One pass,
    /// O(wanted × takes).
    pub(super) fn worth_until(&self, budget: usize) -> usize {
        let mut held = 0;
        for (i, w) in self.wanted.iter().enumerate() {
            if held > budget {
                return i;
            }
            held += self.take(&w.path).map_or(0, |t| t.bytes());
        }
        self.wanted.len()
    }

    /// Whether the loader should work on `path`: it is wanted, and the wanted takes before it don't pass the budget
    /// yet.
    pub(super) fn worth_loading(&self, path: &str, budget: usize) -> bool {
        let mut held = 0;
        for w in &self.wanted {
            if held > budget {
                return false;
            }
            if w.path == path {
                return true;
            }
            held += self.take(&w.path).map_or(0, |t| t.bytes());
        }
        false
    }
}

/// Starts the loader unless it runs already or nothing is queued.
pub(super) fn start(shared: &Arc<Shared>, s: &mut State) {
    if s.worker || s.queue.is_empty() {
        return;
    }
    s.worker = true;
    let shared = shared.clone();
    let spawned = std::thread::Builder::new()
        .name("klipolo-cursor".into())
        .spawn(move || work(&shared));
    if let Err(error) = spawned {
        s.worker = false;
        tracing::warn!(%error, "cursor thread");
    }
}

/// `path`'s take with what `want` asks built, its activity read from `shared`'s store now: `existing`, or read now.
/// None (said once) when the file can't be read.
pub(super) fn job(
    path: &str,
    existing: Option<Arc<TakeCursor>>,
    want: &Want,
    shared: &Shared,
) -> Option<Arc<TakeCursor>> {
    let done = catch_unwind(AssertUnwindSafe(|| {
        let take = match existing {
            Some(take) => take,
            None => Arc::new(load_take(Path::new(path)).map_err(|error| {
                tracing::warn!(file = %crate::log_name(path), %error, "cursor file: not read, no cursor drawn");
            })?),
        };
        take.prepare(&want.smoothings);
        take.prepare_follow(&want.crops, &shared.screens(want), want.aspect);
        Ok::<_, ()>(take)
    }));
    match done {
        Ok(take) => take.ok(),
        Err(_) => {
            tracing::error!(file = %crate::log_name(path), "cursor file: its take failed to build");
            None
        }
    }
}

/// The loading thread: the queued takes in order, at utility QoS, until the queue is empty.
fn work(shared: &Shared) {
    crate::qos::set_current(crate::qos::Qos::Utility);
    loop {
        let (path, existing, want, generation) = {
            let mut s = lock(&shared.state);
            let (path, want) = loop {
                let Some(path) = s.queue.pop_front() else {
                    s.worker = false;
                    s.loading = None;
                    shared.ready.notify_all();
                    return;
                };
                let want = s.wanted.iter().find(|w| *w.path == *path);
                if let Some(want) = want.filter(|_| s.worth_loading(&path, shared.budget)) {
                    break (path, want.clone());
                }
            };
            s.loading = Some(path.clone());
            let existing = s.take(&path).cloned();
            (path, existing, want, s.generation)
        };
        let take = job(&path, existing, &want, shared);
        let gone = {
            let mut s = lock(&shared.state);
            if s.loading.as_ref() == Some(&path) {
                s.loading = None;
            }
            (s.generation == generation).then(|| s.put(path, take, shared.budget))
        };
        // Freed here, on the loader, after the lock.
        drop(gone);
        shared.loaded.notify_one();
        shared.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use klipolo_cursor::CursorTrack;

    use super::*;

    fn want(path: &str) -> Want {
        Want {
            path: path.into(),
            ..Want::default()
        }
    }

    #[test]
    fn a_take_past_the_budget_is_never_queued() {
        // With a budget of nothing, the loaded take "a" passes it, so "b" after it is never loaded and
        // spawns no loader on each publish; "c" before it would still load. The cut is found once.
        let mut s = State::default();
        let take = TakeCursor::new(CursorTrack::default(), PathBuf::new());
        take.prepare_follow(&[], &Default::default(), 0.0);
        let print = take.activity();
        s.takes.push(("a".into(), Some(Arc::new(take))));
        s.wanted = vec![want("c"), want("a"), want("b")];
        assert_eq!(s.worth_until(0), 2, "c and a");
        assert!(s.needs(&want("c"), print));
        assert!(!s.needs(&want("a"), print), "loaded with its paths");
    }
}
