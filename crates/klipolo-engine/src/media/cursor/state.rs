//! The cursor cache's state under its lock: the takes held, least recently used first, what the project
//! wants, and the loader's queue; `put` keeps them within the budget, and `settled` says when the loader has done what
//! the budget lets it. Its users are `tracks` (the public cache) and `queue` (the loader thread).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::take::TakeCursor;
use super::want::Want;

#[derive(Default)]
pub(super) struct State {
    /// Least recently used first. None: the file couldn't be read (tried again when it is next wanted).
    pub(super) takes: Vec<(Arc<str>, Option<Arc<TakeCursor>>)>,
    /// The project's takes in timeline order, with their smoothings (`want`).
    pub(super) wanted: Vec<Want>,
    /// Takes to load, or to smooth again (what their clips draw with changed).
    pub(super) queue: VecDeque<Arc<str>>,
    pub(super) loading: Option<Arc<str>>,
    pub(super) worker: bool,
    /// Bumped by `clear`: a job from before it is dropped.
    pub(super) generation: u64,
}

impl State {
    /// `path`'s entry: Some(None) when its file couldn't be read.
    fn entry(&self, path: &str) -> Option<&Option<Arc<TakeCursor>>> {
        self.takes
            .iter()
            .find(|(p, _)| &**p == path)
            .map(|(_, t)| t)
    }

    pub(super) fn take(&self, path: &str) -> Option<&Arc<TakeCursor>> {
        self.entry(path)?.as_ref()
    }

    /// Holds `take` for `path` as the most recently used, then `trim`s. The takes that go come back, for the caller to
    /// free after the lock.
    pub(super) fn put(
        &mut self,
        path: Arc<str>,
        take: Option<Arc<TakeCursor>>,
        budget: usize,
    ) -> Gone {
        let mut gone = Gone::default();
        if let Some(i) = self.takes.iter().position(|(p, _)| *p == path) {
            gone.0.extend(self.takes.remove(i).1);
        }
        self.takes.push((path, take));
        gone.0.extend(self.trim(budget).0);
        gone
    }

    /// Keeps the takes within the budget: while they pass it, those no longer wanted go, least recently used first;
    /// then every wanted take the loader wouldn't load now goes (the wanted takes before it pass the budget), so the
    /// budget is a hard cap and the preview holds what export's `load_all` would. What goes comes back, as
    /// `put`'s. O(wanted × takes).
    pub(super) fn trim(&mut self, budget: usize) -> Gone {
        let mut gone = Gone::default();
        let total = |s: &Self| {
            s.takes
                .iter()
                .filter_map(|(_, t)| t.as_ref())
                .map(|t| t.bytes())
                .sum::<usize>()
        };
        while total(self) > budget {
            let wanted = |p: &str| self.wanted.iter().any(|w| w.path == p);
            let Some(i) = self
                .takes
                .iter()
                .position(|(p, t)| t.is_some() && !wanted(p))
            else {
                break;
            };
            gone.0.extend(self.takes.remove(i).1);
        }
        let mut held = 0;
        let Some(past) = self.wanted.iter().position(|w| {
            held += self.take(&w.path).map_or(0, |t| t.bytes());
            held > budget
        }) else {
            return gone;
        };
        let after = &self.wanted[past + 1..];
        let mut i = 0;
        while i < self.takes.len() {
            let (p, t) = &self.takes[i];
            if t.is_some() && after.iter().any(|w| *w.path == **p) {
                gone.0.extend(self.takes.remove(i).1);
            } else {
                i += 1;
            }
        }
        gone
    }

    /// Every take of `wants` the loader will hold is in with exactly its paths for the activity `prints` gives (one a
    /// want), or its file couldn't be read: the wanted prefix within the budget, as the loader reads it.
    pub(super) fn settled(&self, wants: &[Want], prints: &[u64], budget: usize) -> bool {
        let mut held = 0;
        for (w, &print) in wants.iter().zip(prints) {
            if held > budget {
                return true;
            }
            match self.entry(&w.path) {
                Some(Some(take)) if take.ready_for(w, print) => held += take.bytes(),
                Some(None) => {}
                _ => return false,
            }
        }
        true
    }
}

pub(super) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Takes the cache let go of (a track and its paths can be megabytes each): freed by whoever holds this once the lock
/// is released, never under it; `free_elsewhere` frees them off the actor.
#[derive(Default)]
#[must_use = "free what goes after the lock"]
pub(super) struct Gone(pub(super) Vec<Arc<TakeCursor>>);

impl Gone {
    /// Frees them on a thread of their own, so the actor never does (its `want` and a project switch's `clear`). A take
    /// still held elsewhere (a zoom build reading it) is freed by its last holder instead.
    pub(super) fn free_elsewhere(self) {
        if self.0.is_empty() {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("klipolo-cursor-free".into())
            .spawn(move || {
                crate::qos::set_current(crate::qos::Qos::Utility);
                drop(self);
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "cursor free thread");
        }
    }
}
