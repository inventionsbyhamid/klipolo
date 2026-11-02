//! Takes an earlier run of Klipolo left behind, added as it opens again. A take outlives Klipolo when it quits from the
//! Dock's menu, crashes or logs out during the take, or when a quit's wait for the save runs out: its worker records on
//! and saves it in its folder, but nothing lands it.
//!
//! - **Which** (`left_in`): each take in the recordings folder (`recover::scan`) set up before this run and at most a
//!   week ago, saved (`Ready`) or cut off (`Interrupted`), not added yet (its `imported` mark, its journal's
//!   `Imported` line) and not tried before (its `not-recovered` mark): the newest `AT_MOST`.
//! - **How** (`recover_left`): oldest first, so the newest shows last, each lands in a new project through the path a
//!   finished take takes (`Sink::finished` with `ImportTarget::New`). A cut-off take is finished from its files first
//!   (`recover::repair`, which takes the take's own lock), before it is handed on, so the landing waits on no repair.
//!   The page hears it as `recorder-imported` with `earlier_run`, and says "Recovered Recording 1 from your last
//!   session."
//! - **Never twice:** a landed take has its `imported` mark. A take whose `session.json` can't be read is skipped by
//!   the scan, with a log line.
//! - **One that doesn't land** is logged and told (`Sink::left`, `LeftOutcome::NotAdded`), its files left in its
//!   folder. When that lasts (nothing to edit, files that can't be finished: `outcome_of`) it is marked
//!   `not-recovered`, so no launch tries it again; otherwise (the open project can't be saved, the library takes no new
//!   project) the next launch tries again. One added whose new project couldn't be opened is told so
//!   (`LeftOutcome::AddedNotOpened`).
//! - **Still recording** (`Live`): an earlier run's worker records on. The person is told once
//!   (`LeftOutcome::StillRecording`); it is looked at again every `every` and lands once its worker has let it go (the
//!   person stopped it from the menu bar).
//!
//! The app runs it on a thread of its own, once per run, when the page asks (`recorder_recover_left`): the page follows
//! the recorder by then, so it hears each take, and Klipolo's launch never waits for it.

use std::path::Path;
use std::time::Duration;

use klipolo_capture::recover::{self, Found, FoundState, RecoverError, SCAN_LIMIT};
use klipolo_capture::session::{ImportTarget, SessionDir};
use klipolo_model::coded::CodedError;

use super::env::{Done, Sink};
use super::types::LeftOutcome;

/// The most takes one launch adds: a folder of old takes never floods the library. The next launch adds the rest.
pub(super) const AT_MOST: usize = 5;
/// How long ago a take may have been set up and still be added: a week.
pub(super) const WITHIN_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// The mark a take that can never land gets, in its folder, so no launch tries it again.
pub(super) const NOT_RECOVERED: &str = "not-recovered";
/// How often the app looks again at a take an earlier run's worker still records.
pub(super) const WATCH_EVERY: Duration = Duration::from_secs(2);

/// The takes left behind in the recordings folder.
#[derive(Debug, Default)]
pub(super) struct Left {
    /// Saved or cut off: to land now, newest first.
    pub(super) land: Vec<Found>,
    /// An earlier run's worker records them still.
    pub(super) live: Vec<Found>,
}

/// The takes in `root` an earlier run left behind (the module doc): set up before `before_ms` (this run's launch)
/// and at most `WITHIN_MS` before `now_ms`, not added and not tried before.
pub(super) fn left_in(root: &Path, before_ms: u64, now_ms: u64) -> Left {
    let mut left = Left::default();
    for found in recover::scan(root, SCAN_LIMIT) {
        let earlier = found.started_ms < before_ms;
        if !earlier || now_ms.saturating_sub(found.started_ms) > WITHIN_MS || tried(&found) {
            continue;
        }
        match found.state {
            FoundState::Ready | FoundState::Interrupted if left.land.len() < AT_MOST => {
                left.land.push(found);
            }
            FoundState::Live => left.live.push(found),
            _ => {}
        }
    }
    left
}

/// Lands each take `left_in` finds through `sink`, oldest first, then each one an earlier run's worker still records
/// once it has let it go, looked at every `every`, the person told it is still going first. How many landed. Returns
/// once none is left to wait for.
pub(super) fn recover_left(
    root: &Path,
    sink: &dyn Sink,
    before_ms: u64,
    now_ms: u64,
    every: Duration,
) -> usize {
    let Left { land, mut live } = left_in(root, before_ms, now_ms);
    if !land.is_empty() || !live.is_empty() {
        tracing::info!(
            to_land = land.len(),
            recording = live.len(),
            "recorder: takes from an earlier run were found"
        );
    }
    for found in &live {
        sink.left(&found.id, LeftOutcome::StillRecording);
    }
    let mut landed = 0;
    for found in land.iter().rev() {
        landed += usize::from(land_one(found, sink));
    }
    while !live.is_empty() {
        std::thread::sleep(every);
        live.retain_mut(|found| {
            found.state = recover::state_of(&SessionDir::at(&found.dir));
            match found.state {
                FoundState::Live => true,
                FoundState::Ready | FoundState::Interrupted => {
                    landed += usize::from(land_one(found, sink));
                    false
                }
                FoundState::Imported => false,
            }
        });
    }
    landed
}

/// Lands `found` in a new project through `sink`, as a take from the start screen lands, told as one from an earlier
/// run; a cut-off one finished from its files first. True when it landed. When it didn't, the person is told, and it
/// is marked `not-recovered` when that lasts (`outcome_of`).
fn land_one(found: &Found, sink: &dyn Sink) -> bool {
    let dir = SessionDir::at(&found.dir);
    tracing::info!(
        take = found.id,
        state = ?found.state,
        "recorder: a take from an earlier run lands in a new project"
    );
    let interrupted = found.state == FoundState::Interrupted;
    let landed = finish(&dir, interrupted).and_then(|()| {
        sink.finished(&Done {
            id: found.id.clone(),
            dir: dir.clone(),
            target: ImportTarget::New,
            project_file: None,
            recover: interrupted,
            note: None,
            earlier_run: true,
        })
    });
    let Err(error) = landed else {
        return true;
    };
    let (outcome, lasting) = outcome_of(&error);
    if outcome == LeftOutcome::AddedNotOpened {
        sink.left(&found.id, outcome);
        return true;
    }
    if lasting {
        tracing::warn!(
            take = found.id,
            code = error.code,
            "recorder: a take from an earlier run couldn't be added, so it stays in its folder and isn't tried again"
        );
        if let Err(error) = std::fs::write(found.dir.join(NOT_RECOVERED), b"") {
            tracing::warn!(%error, take = found.id, "recorder: the take's not-recovered mark wasn't written");
        }
    } else {
        tracing::warn!(
            take = found.id,
            code = error.code,
            "recorder: a take from an earlier run couldn't be added for now, so the next launch tries again"
        );
    }
    sink.left(&found.id, outcome);
    false
}

/// A cut-off take (`interrupted`) finished from its files (`recover::repair`, idempotent, under the take's own lock),
/// so its landing needs no repair; a saved one as it is. `capture.recovered.failed` when its files can't be finished,
/// and `capture.import.notAdded` when a worker holds it after all (the next launch tries again).
fn finish(dir: &SessionDir, interrupted: bool) -> Result<(), CodedError> {
    if !interrupted {
        return Ok(());
    }
    match recover::repair(dir) {
        Ok(repaired) => {
            tracing::info!(
                took_ms = repaired.took_ms,
                "recorder: a take from an earlier run was finished from its files"
            );
            Ok(())
        }
        Err(RecoverError::Live) => Err(CodedError::new("capture.import.notAdded")),
        Err(error) => {
            tracing::error!(%error, "recorder: a take from an earlier run couldn't be finished from its files");
            Err(CodedError::new("capture.recovered.failed"))
        }
    }
}

/// What the person is told about a take an earlier run left behind that didn't land as a `recorder-imported`, from what
/// its landing said (`error`), and whether that lasts: a take with nothing to edit (`capture.import.noVideo`,
/// `capture.import.videoUnreadable`) or whose files can't be finished (`capture.recovered.failed`) never lands, and is
/// marked. Anything else may pass, and the next launch tries again. One added whose project couldn't be opened
/// (`capture.import.addedNotOpened`) is said as such, and is added.
pub(super) fn outcome_of(error: &CodedError) -> (LeftOutcome, bool) {
    const LASTING: [&str; 3] = [
        "capture.import.noVideo",
        "capture.import.videoUnreadable",
        "capture.recovered.failed",
    ];
    match error.code.as_str() {
        "capture.import.addedNotOpened" => (LeftOutcome::AddedNotOpened, false),
        code => (LeftOutcome::NotAdded, LASTING.contains(&code)),
    }
}

/// A launch found earlier that `found` can never land.
fn tried(found: &Found) -> bool {
    found.dir.join(NOT_RECOVERED).exists()
}
