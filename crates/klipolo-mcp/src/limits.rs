//! What an app may do in a while, and what apps may hold of the person's undo history.
//!
//! Paces, kept in memory for this run, each over a window that slides with the clock:
//! - an app's edit calls (`CALLS_PER_MINUTE`), checked before anything else of the call;
//! - batches applied at once (`APPLIED_PER_WINDOW` for one app, `APPLIED_ALL_PER_WINDOW` for all apps together, so a
//!   new name doesn't start over);
//! - sessions opened (`OPENED_PER_MINUTE` for one app, `OPENED_ALL_PER_MINUTE` for all apps together, so names an app
//!   picks itself can't push out the sessions of apps already connected);
//! - jobs that cost money (`PAID_JOBS_PER_WINDOW` for one app), within what all apps together may spend in this run of
//!   Klipolo (`PAID_RUN_CAP_USD`, by the host's estimates). Only `Host::start_paid` starts one, after the app's own
//!   checks; one it refused gives its price back, since nothing was spent.
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

/// Jobs that cost money one app may start within `PAID_WINDOW`.
pub const PAID_JOBS_PER_WINDOW: usize = 10;
pub const PAID_WINDOW: Duration = Duration::from_secs(10 * 60);
/// What all apps together may spend on jobs in one run of Klipolo, in US dollars; past it, the person continues in
/// Klipolo's own chat.
pub const PAID_RUN_CAP_USD: f64 = 0.25;

const MINUTE: Duration = Duration::from_secs(60);
/// Apps whose paces are kept before those with nothing in their windows go.
const APPS_KEPT: usize = 64;

/// Why a call waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paced {
    /// The app, or all apps together, reached a pace: it waits this long, until the oldest event that holds it back
    /// leaves the window, and tries again.
    Wait(Duration),
    /// Apps spent what they may on jobs in this run (`PAID_RUN_CAP_USD`).
    OverCap,
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

/// How long until `times` hold fewer than `limit` within `window` of the clock, when they hold `limit` or more at
/// `now`: until the one that must leave for room does.
fn full(
    times: &mut VecDeque<Instant>,
    now: Instant,
    window: Duration,
    limit: usize,
) -> Option<Duration> {
    let held = recent(times, now, window);
    (held >= limit).then(|| (times[held - limit] + window).saturating_duration_since(now))
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
        let wait = full(mine, now, window, each)
            .max(all.and_then(|all| full(&mut self.all, now, window, all)));
        if let Some(wait) = wait {
            return Err(Paced::Wait(wait));
        }
        if all.is_some() {
            self.all.push_back(now);
        }
        mine.push_back(now);
        Ok(())
    }

    /// Takes back the event `take` recorded for `client` at `at`. O(events in the window).
    fn give_back(&mut self, client: &str, at: Instant) {
        if let Some(times) = self.apps.get_mut(client)
            && let Some(i) = times.iter().rposition(|t| *t == at)
        {
            times.remove(i);
        }
        if let Some(i) = self.all.iter().rposition(|t| *t == at) {
            self.all.remove(i);
        }
    }
}

#[derive(Default)]
struct Paces {
    calls: Pace,
    applied: Pace,
    opened: Pace,
    paid: Pace,
    /// What jobs started in this run cost, all apps together, in millionths of a dollar.
    spent: u64,
}

/// `usd` in millionths of a dollar; None when it isn't a number (never under an allowance). Below zero is nothing.
fn micros(usd: f64) -> Option<u64> {
    usd.is_finite().then(|| (usd.max(0.0) * 1e6).round() as u64)
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

    /// Gives back the place `applied` took for `client` at `at`: the host refused that batch, so nothing applied.
    pub fn unapplied(&self, client: &str, at: Instant) {
        self.paces().applied.give_back(client, at);
    }

    /// One more job of `usd` for `client` at `now`: `OverCap` when it would take all apps' spending in this run past
    /// `PAID_RUN_CAP_USD`, `Wait` past the app's pace of jobs. Counted as spent when it passes.
    pub fn paid(&self, client: &str, now: Instant, usd: f64) -> Result<(), Paced> {
        let cost = micros(usd).ok_or(Paced::OverCap)?;
        let cap = micros(PAID_RUN_CAP_USD).unwrap_or(0);
        let mut paces = self.paces();
        if paces.spent.saturating_add(cost) > cap {
            return Err(Paced::OverCap);
        }
        (paces.paid).take(client, now, PAID_WINDOW, (PAID_JOBS_PER_WINDOW, None))?;
        paces.spent += cost;
        Ok(())
    }

    /// Gives back what `paid` counted for a job of `usd` that didn't start (the host's own checks refused it). The
    /// app's pace of jobs keeps it: an app looping on refused jobs is still paced.
    pub fn unpaid(&self, usd: f64) {
        let cost = micros(usd).unwrap_or(0);
        let mut paces = self.paces();
        paces.spent = paces.spent.saturating_sub(cost);
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
