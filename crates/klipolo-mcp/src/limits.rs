//! What an app may do in a while, and what apps may hold of the person's undo history.
//!
//! Paces, kept in memory for this run, each over a window that slides with the clock:
//! - an app's edit calls (`CALLS_PER_MINUTE`), checked before anything else of the call;
//! - batches applied at once (`APPLIED_PER_WINDOW` for one app, `APPLIED_ALL_PER_WINDOW` for all apps together, so a
//!   new name doesn't start over);
//! - sessions opened (`OPENED_PER_MINUTE` for one app, `OPENED_ALL_PER_MINUTE` for all apps together, so names an app
//!   picks itself can't push out the sessions of apps already connected).
//!
//! Apps are known by the name they give (`session::client_name`): sessions of one name share their paces.
//!
//! The undo history: an app's batches commit with `Document::commit_tx_keeping` within the apps' shares
//! (`undo_shares`): an app's steps hold at most a quarter of the history's steps and bytes, all apps' together half.
//! Room is made only from the app's own steps, so an app's step never pushes out one the person made, or their redo.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use klipolo_model::history::{HistoryLimits, Share, Whose};

/// Edit calls one app may make within a minute.
pub const CALLS_PER_MINUTE: usize = 60;
/// Batches one app may apply at once within `PACE_WINDOW`.
pub const APPLIED_PER_WINDOW: usize = 30;
/// Batches all apps together may apply at once within `PACE_WINDOW`.
pub const APPLIED_ALL_PER_WINDOW: usize = 60;
pub const PACE_WINDOW: Duration = Duration::from_secs(5 * 60);
/// Sessions one app may open within a minute.
pub const OPENED_PER_MINUTE: usize = 10;
/// Sessions all apps together may open within a minute: below `session::MAX_SESSIONS`, so a minute of new names never
/// replaces every session kept.
pub const OPENED_ALL_PER_MINUTE: usize = 20;

const MINUTE: Duration = Duration::from_secs(60);
/// Apps whose paces are kept before those with nothing in their windows go.
const APPS_KEPT: usize = 64;

/// Why a call waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paced {
    /// The app, or all apps together, reached a pace: it waits a little and tries again.
    Wait,
}

/// When recent events of one kind happened, oldest first: each app's, and all apps' together.
#[derive(Default)]
struct Pace {
    apps: HashMap<String, VecDeque<Instant>>,
    all: VecDeque<Instant>,
}

/// How many of `times` fall within `window` before `now`, dropping the older ones.
fn recent(times: &mut VecDeque<Instant>, now: Instant, window: Duration) -> usize {
    while (times.front()).is_some_and(|t| now.saturating_duration_since(*t) >= window) {
        times.pop_front();
    }
    times.len()
}

impl Pace {
    /// One more event for `client` at `now`, unless it had `each` within `window`, or all apps `all` (when given).
    /// Nothing is recorded when it waits. O(apps) when a new app comes past `APPS_KEPT`, else O(1) amortized.
    fn take(
        &mut self,
        client: &str,
        now: Instant,
        window: Duration,
        (each, all): (usize, Option<usize>),
    ) -> Result<(), Paced> {
        if self.apps.len() >= APPS_KEPT && !self.apps.contains_key(client) {
            self.apps.retain(|_, times| recent(times, now, window) > 0);
        }
        let mine = self.apps.entry(client.to_owned()).or_default();
        if recent(mine, now, window) >= each {
            return Err(Paced::Wait);
        }
        if let Some(all) = all {
            if recent(&mut self.all, now, window) >= all {
                return Err(Paced::Wait);
            }
            self.all.push_back(now);
        }
        mine.push_back(now);
        Ok(())
    }
}

#[derive(Default)]
struct Paces {
    calls: Pace,
    applied: Pace,
    opened: Pace,
}

/// The paces of this run, shared by every session.
#[derive(Default)]
pub struct Limits {
    paces: Mutex<Paces>,
}

impl Limits {
    fn paces(&self) -> MutexGuard<'_, Paces> {
        self.paces.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One more edit call from `client` at `now`.
    pub fn call(&self, client: &str, now: Instant) -> Result<(), Paced> {
        (self.paces().calls).take(client, now, MINUTE, (CALLS_PER_MINUTE, None))
    }

    /// One more session opened by `client` at `now`.
    pub fn opened(&self, client: &str, now: Instant) -> Result<(), Paced> {
        let limits = (OPENED_PER_MINUTE, Some(OPENED_ALL_PER_MINUTE));
        self.paces().opened.take(client, now, MINUTE, limits)
    }

    /// One more batch applied at once for `client` at `now`.
    pub fn applied(&self, client: &str, now: Instant) -> Result<(), Paced> {
        let limits = (APPLIED_PER_WINDOW, Some(APPLIED_ALL_PER_WINDOW));
        self.paces().applied.take(client, now, PACE_WINDOW, limits)
    }
}

/// The shares of `l` that `client`'s steps (a quarter) and all apps' steps together (half) may hold, by steps and
/// bytes.
pub fn undo_shares(client: &str, l: HistoryLimits) -> [Share<'_>; 2] {
    [
        Share {
            of: Whose::App(client),
            steps: l.steps / 4,
            bytes: l.bytes / 4,
        },
        Share {
            of: Whose::Apps,
            steps: l.steps / 2,
            bytes: l.bytes / 2,
        },
    ]
}
