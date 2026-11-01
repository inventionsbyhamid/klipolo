//! Recovery: unfinished takes, found and finished from the files alone after a crash, a `kill -9` or
//! a power cut.
//!
//! - **`scan`** lists the takes in the recordings folder, newest first: live (a recorder holds the lock), imported,
//!   ready (finalized but not imported) or interrupted. It reads only `session.json`, takes the lock for an instant and
//!   reads the journal's last 64 KB.
//! - **`repair`** finishes an interrupted take so it can be imported:
//!   1. it takes the lock, retrying for about 500 ms first: a recorder that is starting or resuming may hold it for a
//!      moment, and so may Windows for a killed one;
//!   2. it reads the config (none: `NoConfig`), the journal (a torn last line is normal; none at all means "timing
//!      approximate") and the anchors;
//!   3. it repairs each run file the journal's `RunStarted` lines name, or else every `<source>/run-NN.mp4|caf` in the
//!      folder: video by `repair_video`, sound by `repair_sound` (their docs give the steps);
//!   4. it derives the manifest as Stop does, marks it `recovered`, writes it, and journals `Recovered`.
//!
//! **What it trusts.** A file's durable length is the most any `Durable` line gives it (`JournalRead::durable_through`:
//! a run closed in an earlier tick isn't named by later lines). Up to there, the file is what the recorder wrote: the
//! tick ordered its media ahead of the line, and repair's own `Durable` line follows the same order (the media pushed
//! to the disk first). Past it, bytes are kept only when they show they landed whole (a video
//! fragment's samples are whole NAL units; a sound frame isn't all zeros), so a kill, where everything handed to
//! `write()` reached the disk, loses nothing more, and a power cut's zeroed pages are never imported.
//!
//! **What it never does.** It never changes a fragment or a frame it keeps, never pads, and never deletes what it
//! can't keep: the bytes it cuts lie past the damage, and those that aren't zeros are copied beside the file
//! (`<file>.damaged`) first. A non-zero damaged tail over 64 MB isn't a power cut's last second but footage after
//! corruption, so that run is left exactly as it is and out of the manifest. Nor does it lose part of a take
//! silently: every run left out with something recorded in it (`RunOutcome::LeftOut`) is named in the manifest's notes
//! (`capture.recovered.partKept`, or `sourceKept` when that was every run of its source), and its files stay in the
//! take's folder. Every step is idempotent: a repair cut
//! short by another crash is finished by the next, and a repeated repair changes nothing.
//!
//! **One bad run never stops the take**: a run whose file fails with an I/O error (a bad sector, a
//! permission) is left out and named like the others; only a full or read-only disk, or the journal itself failing,
//! stops the repair, and a later one goes on. **Long repairs** report their progress and can be cancelled
//! (`repair_watched`): a take whose journal is lost reads every sample.
//!
//! The cursor file is a line file: trusted to its durable length, then its whole lines that read (`cursor`).
//!
//! A sound run a crash ended gets its per-channel levels journaled (`Entry::Levels`), as Stop gives a finished run's:
//! from its last running level (journaled each tick) over the frames after it, so a long take is never read whole for
//! them, and a take without a journal gets none. A file that can't be read for them gets a mild note instead
//! (`capture.note.levels`): levels are a hint for import.
//! The resumed recorder and Stop's slow path finish run files with the same per-file step (`finish_file`).

mod cursor;
mod damage;
mod head;
mod nal;
mod runs;
mod scan;
mod sound;
mod steps;
mod types;
mod video;

pub use scan::{Ending, Found, FoundState, SCAN_LIMIT, ending, scan, state_of};
pub use sound::repair_sound;
use types::Unwatched;
pub use types::{RecoverError, RepairWatch, Repaired, RunOutcome, RunRepair};
pub use video::repair_video;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, ErrorKind};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use klipolo_platform::durable::Durability;

use crate::english::msg;
use crate::io::{Io, OsIo};
use crate::journal::{self, Entry, JournalRead};
use crate::manifest;
use crate::session::{self, SessionDir};
pub(crate) use cursor::finish_cursor;
pub(crate) use runs::{RunFile, told};
use runs::{found, placed, start_us};
pub(crate) use steps::stops_the_take;
pub(crate) use steps::{Lines, finish_file, hold, leveled, levels_or_note, running};

/// How long `repair` retries a held lock before it calls the take live.
pub(crate) const LOCK_WAIT: Duration = Duration::from_millis(500);
pub(crate) const LOCK_RETRY: Duration = Duration::from_millis(25);

/// Finishes the take in `dir` (the module doc). Idempotent.
pub fn repair(dir: &SessionDir) -> Result<Repaired, RecoverError> {
    repair_with(dir, &OsIo)
}

/// `repair` with every write and sync to the take's journal and run files going through `io`, so a test can log them
/// and replay a power cut in the middle of a repair (`LoggedIo`).
pub fn repair_with(dir: &SessionDir, io: &dyn Io) -> Result<Repaired, RecoverError> {
    repair_watched(dir, io, &mut Unwatched)
}

/// `repair_with`, reporting progress to `watch` and stopping with `Cancelled` when it says so.
pub fn repair_watched(
    dir: &SessionDir,
    io: &dyn Io,
    watch: &mut dyn RepairWatch,
) -> Result<Repaired, RecoverError> {
    let started = Instant::now();
    let _lock = hold(dir)?;
    let cfg = match session::read_config(dir) {
        Ok(cfg) => cfg,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::InvalidData) => {
            return Err(RecoverError::NoConfig);
        }
        Err(e) => return Err(failed(e, "session.json")),
    };
    let mut journal = match journal::read(&dir.journal()) {
        Ok(read) => read,
        Err(e) if e.kind() == ErrorKind::NotFound => JournalRead::default(),
        Err(e) => return Err(failed(e, "journal.jsonl")),
    };
    let anchors = journal::read_anchors(&dir.timing()).map_err(|e| failed(e, "timing.jsonl"))?;
    let durable = journal.durable_through();
    let mut runs = told(&journal);
    let approximate = runs.is_empty();
    if approximate {
        runs = found(dir, &cfg).map_err(|e| failed(e, "the run folders"))?;
    }
    let mut lines = Lines::new(io, dir.journal());
    let (mut durations, mut damaged_bytes, mut changed) = (BTreeMap::new(), 0, false);
    let mut left_out: Vec<&RunFile> = Vec::new();
    let leveled: BTreeSet<String> = leveled(&journal);
    let running = running(&journal);
    let paths: Vec<Option<PathBuf>> = runs.iter().map(|r| dir.key_path(&r.file).ok()).collect();
    // Each file's size as found: progress counts what there was to check, whatever repair then cuts or adds.
    let sizes: Vec<u64> = paths
        .iter()
        .map(|p| {
            p.as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map_or(0, |m| m.len())
        })
        .collect();
    let total: u64 = sizes.iter().sum();
    let mut done = 0;
    for ((run, path), size) in runs.iter().zip(&paths).zip(&sizes) {
        let Some(path) = path else {
            tracing::warn!(
                file = run.file,
                "recovery: skipped a run file outside the take"
            );
            continue;
        };
        if watch.cancelled() {
            return Err(RecoverError::Cancelled);
        }
        let bound = durable.get(&run.file).copied();
        let mut step = |n: u64| {
            watch.progress(done + n, total);
            !watch.cancelled()
        };
        let outcome = finish_file(io, path, run, bound, &mut lines, &mut step);
        done += size;
        match outcome {
            Ok(RunOutcome::Repaired(r)) => {
                durations.insert(run.file.clone(), r.duration_us);
                damaged_bytes += r.damaged_bytes;
                changed |= r.changed;
                // A take without its journal has no running levels: its whole files are never read for them.
                if !approximate && !run.video && !leveled.contains(&run.file) {
                    let mut step = |_| {
                        watch.progress(done, total);
                        !watch.cancelled()
                    };
                    let from = running.get(&run.file);
                    let Ok(entry) =
                        levels_or_note(path, run, cfg.label_of(&run.source), from, &mut step)
                    else {
                        return Err(RecoverError::Cancelled);
                    };
                    if let Some(entry) = entry {
                        let written = lines.write(&entry, Durability::Ordered);
                        written.map_err(|e| failed(e, "journal.jsonl"))?;
                        journal.entries.push(entry);
                    }
                }
            }
            Ok(RunOutcome::Empty) => {}
            Ok(RunOutcome::LeftOut) => left_out.push(run),
            Err(_) if watch.cancelled() => return Err(RecoverError::Cancelled),
            Err(e) if lines.failed || stops_the_take(&e) => return Err(failed(e, &run.file)),
            Err(e) => {
                tracing::error!(file = run.file, error = %e, "recovery: a run's file failed, so it is left out");
                left_out.push(run);
            }
        }
        watch.progress(done, total);
    }
    // The cursor file, to its durable length and its last whole line (its shapes are whole or absent).
    if let Err(e) = finish_cursor(io, dir, durable.get(session::CURSOR).copied()) {
        if lines.failed || stops_the_take(&e) {
            return Err(failed(e, session::CURSOR));
        }
        tracing::error!(error = %e, "recovery: the cursor file couldn't be finished, so it is left as it is");
    }
    if approximate {
        journal.entries.extend(placed(&runs, &durations));
    }
    let finalized = journal
        .entries
        .iter()
        .any(|e| matches!(e, Entry::Finalized { .. }));
    let mut manifest = manifest::derive(&cfg, &journal, &anchors, &durations);
    for run in &left_out {
        let source = manifest::source_param(cfg.label_of(&run.source));
        // Every run of the source left out: the whole source, not "Part of" it.
        let whole = !(manifest.sources.iter()).any(|s| s.id == run.source && !s.runs.is_empty());
        let note = if whole {
            msg!("capture.recovered.sourceKept", source = source)
        } else {
            let at_us = start_us(&runs, &durations, run);
            msg!("capture.recovered.partKept", source = source, atUs = at_us)
        };
        if !manifest.notes.contains(&note) {
            manifest.notes.push(note);
        }
    }
    if approximate {
        for run in manifest.sources.iter_mut().flat_map(|s| &mut s.runs) {
            run.approximate = true;
        }
        let note = msg!("capture.recovered.approximate");
        if !manifest.notes.contains(&note) {
            manifest.notes.push(note);
        }
    }
    // A take Stop finished and nothing needed repairing is as Stop left it.
    manifest.recovered |= changed || !finalized || !left_out.is_empty();
    manifest::write_manifest(dir, &manifest).map_err(|e| failed(e, "manifest.json"))?;
    let journaled = journal
        .entries
        .iter()
        .any(|e| matches!(e, Entry::Recovered { .. }));
    if manifest.recovered && !journaled {
        let at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        lines
            .write(&Entry::Recovered { at_ms }, Durability::Full)
            .map_err(|e| failed(e, "journal.jsonl"))?;
    }
    let took_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
    tracing::info!(
        take = cfg.id,
        took_ms,
        damaged_bytes,
        changed,
        approximate,
        "recovery: repaired"
    );
    Ok(Repaired {
        manifest,
        took_ms,
        damaged_bytes,
    })
}

pub(crate) fn failed(e: io::Error, what: &str) -> RecoverError {
    tracing::error!(file = what, error = %e, "recovery: a disk step failed");
    RecoverError::Io(e.kind())
}
