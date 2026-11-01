//! Resume: the app relaunched a crashed worker with `--resume`, so the worker goes on with the same
//! take. Before any pipe starts:
//! 1. **The lock**, retried for about 500 ms (a killed worker's lock may linger a moment on Windows); still held by
//!    another worker: `Live`.
//! 2. **The crashed runs are finished** with recovery's per-file steps (`recover::finish_file`: walk or scan, keep a
//!    `.damaged` tail, truncate, remux or size). Every run the journal names goes through them (each is idempotent, so
//!    a run already finished is only measured), and a run without `RunEnded` gets one at its salvaged end, so a take
//!    that later reads `Finalized` never holds an unfinished run. A run that can't be repaired is named
//!    (`capture.recovered.partKept`, critical) and its files kept. A sound run gets its levels (`Entry::Levels`).
//! 3. **The session clock** is rebuilt from `Gate`, `Pause` and `Resume`; a trailing `Pause` stays paused. The host
//!    clock is continuous: a resume never crosses a reboot or an app relaunch.
//! 4. **New runs** are numbered after each source's highest, and each source's first new sample notes the gap from its
//!    salvaged end (`capture.note.gap`). New markers are numbered after the highest too.
//!
//! The core then starts in Recording (or Paused) with no countdown and no gate: the person already started this take.
//!
//! 5. **The cursor**: its file trusted to its durable length, its writer rebuilt from it (no second header, shape
//!    ids continue), and a take whose journal says the cursor was burned in resumes with it in the screen's frames.
//!
//! **Never after Stop** (capture consent). A journal holding `StopRequested` or `Finalized` means
//! the person ended the take and the worker died in Stop's finishing; one holding `Recovered` was finished by recovery
//! (a Home scan) since the worker died. Either way `Resumed::stopped` is set, and the worker only
//! finishes the take (`run::prepare`), whatever the app asked. No screen, camera or mic opens again.

use std::collections::BTreeMap;

use klipolo_cursor::{CursorTrack, Limits};
use klipolo_model::time::Us;
use klipolo_platform::durable::Durability;

use super::core::SessionCore;
use super::source::Status;
use crate::backend::DeviceInfo;
use crate::clock::SessionClock;
use crate::cursor::CursorWriter;
use crate::english::msg;
use crate::io::{Io, OsIo};
use crate::journal::{self, Entry};
use crate::manifest::source_param;
use crate::recover::{self, Lines, RecoverError, RunOutcome};
use crate::session::{self, FileKey, SessionConfig, SessionDir, SessionLock};
use crate::shell::WorkerState;

/// What a resumed core starts from.
#[derive(Clone, Debug)]
pub struct Resumed {
    pub clock: SessionClock,
    /// Each source's next run number: its highest so far + 1.
    pub next_run: BTreeMap<String, u32>,
    /// Where each source's media ended before the crash, in session µs.
    pub ended_at: BTreeMap<String, Us>,
    /// Every earlier run's length, for the manifest at Stop.
    pub done: BTreeMap<FileKey, Us>,
    /// The person's Stop was journaled (`StopRequested` or `Finalized`), or recovery finished the take (`Recovered`):
    /// it is finished, never recorded again.
    pub stopped: bool,
    /// The highest marker number so far (`m<n>`): new marks go on from it, so none replaces one placed before the crash.
    pub markers: u32,
    /// The cursor as the crashed worker left it.
    pub cursor: ResumedCursor,
}

/// A resumed take's cursor: its writer rebuilt from the file (no second header, shape ids continue,
/// nothing older than each kind's last line), and whether the watchdog had burned the cursor in.
#[derive(Clone, Debug, Default)]
pub struct ResumedCursor {
    /// None: no cursor file (a fresh writer).
    pub writer: Option<CursorWriter>,
    /// The journal's first `CursorBurned`.
    pub burned: Option<Us>,
    /// The journal has `CursorBurnDecided` (or `CursorBurned`): the resumed screen shows the cursor in its frames.
    pub decided: bool,
    /// The file has its `x` line.
    pub file_burned: bool,
}

/// Steps 1-3 for the take in `dir` (the module doc), with the take's lock, which the worker holds from here on.
pub fn resume(dir: &SessionDir, now_ns: u64) -> Result<(SessionLock, Resumed), RecoverError> {
    resume_with(dir, &OsIo, now_ns)
}

pub fn resume_with(
    dir: &SessionDir,
    io: &dyn Io,
    _now_ns: u64,
) -> Result<(SessionLock, Resumed), RecoverError> {
    let lock = recover::hold(dir)?;
    let cfg = session::read_config(dir).map_err(|_| RecoverError::NoConfig)?;
    let journal = match journal::read(&dir.journal()) {
        Ok(read) => read,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => journal::JournalRead::default(),
        Err(e) => return Err(recover::failed(e, "journal.jsonl")),
    };
    let durable = journal.durable_through();
    let ended: BTreeMap<(String, u32), Us> = journal
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::RunEnded {
                source,
                run,
                end_us,
            } => Some(((source.clone(), *run), *end_us)),
            _ => None,
        })
        .collect();
    let leveled = recover::leveled(&journal);
    let running = recover::running(&journal);
    let mut lines = Lines::new(io, dir.journal());
    let mut out = Resumed {
        clock: journal.clock(),
        next_run: BTreeMap::new(),
        ended_at: BTreeMap::new(),
        done: BTreeMap::new(),
        stopped: (journal.entries.iter()).any(|e| {
            matches!(
                e,
                Entry::StopRequested { .. } | Entry::Finalized { .. } | Entry::Recovered { .. }
            )
        }),
        markers: (journal.entries.iter())
            .filter_map(|e| match e {
                Entry::Marker { id, .. } => id.strip_prefix('m')?.parse().ok(),
                _ => None,
            })
            .max()
            .unwrap_or(0),
        cursor: resumed_cursor(io, dir, &journal, durable.get(session::CURSOR).copied()),
    };
    for run in recover::told(&journal) {
        let next = out.next_run.entry(run.source.clone()).or_insert(1);
        *next = (*next).max(run.run + 1);
        let path = dir
            .key_path(&run.file)
            .map_err(|e| recover::failed(e, &run.file))?;
        let bound = durable.get(&run.file).copied();
        let outcome = match recover::finish_file(io, &path, &run, bound, &mut lines, &mut |_| true)
        {
            Ok(outcome) => outcome,
            // One bad run never stops the take: it is named and left out, as recovery leaves it.
            Err(e) if !lines.failed && !recover::stops_the_take(&e) => {
                tracing::error!(file = run.file, error = %e, "resume: a crashed run's file failed, so it is left out");
                RunOutcome::LeftOut
            }
            Err(e) => return Err(recover::failed(e, &run.file)),
        };
        let offset = run.offset_us.unwrap_or(0);
        let label = cfg.label_of(&run.source);
        let length = match outcome {
            RunOutcome::Repaired(r) => {
                out.done.insert(run.file.clone(), r.duration_us);
                if !run.video
                    && !leveled.contains(&run.file)
                    && let Ok(Some(entry)) = recover::levels_or_note(
                        &path,
                        &run,
                        label,
                        running.get(&run.file),
                        &mut |_| true,
                    )
                {
                    write(&mut lines, &entry)?;
                }
                r.duration_us
            }
            RunOutcome::Empty => 0,
            RunOutcome::LeftOut => {
                let note = msg!(
                    "capture.recovered.partKept",
                    source = source_param(label),
                    atUs = offset
                );
                write(
                    &mut lines,
                    &Entry::health(&note, Some(run.source.clone()), true),
                )?;
                0
            }
        };
        let end = match ended.get(&(run.source.clone(), run.run)) {
            Some(&end) => end,
            None => {
                let end = offset + length;
                write(
                    &mut lines,
                    &Entry::RunEnded {
                        source: run.source.clone(),
                        run: run.run,
                        end_us: end,
                    },
                )?;
                end
            }
        };
        let at = out.ended_at.entry(run.source.clone()).or_insert(end);
        *at = (*at).max(offset + length);
    }
    Ok((lock, out))
}

/// The cursor file trusted to its durable length (`recover::finish_cursor`), then read into the resumed writer
/// (streamed: a long take's file is never in memory whole), and the journal's burn-in. A cursor file that can't be
/// finished or read never stops the take (as in recovery): it goes on without the file's past (a fresh writer).
fn resumed_cursor(
    io: &dyn Io,
    dir: &SessionDir,
    journal: &journal::JournalRead,
    durable: Option<u64>,
) -> ResumedCursor {
    let burned = journal.entries.iter().find_map(|e| match e {
        Entry::CursorBurned { at_us } => Some(*at_us),
        _ => None,
    });
    let decided = burned.is_some()
        || (journal.entries.iter()).any(|e| matches!(e, Entry::CursorBurnDecided { .. }));
    let finished = recover::finish_cursor(io, dir, durable);
    let track = finished
        .and_then(|()| std::fs::File::open(dir.cursor()))
        .map(|file| {
            let len = file.metadata().map_or(0, |m| m.len());
            let read = std::io::BufReader::with_capacity(64 * 1024, file);
            CursorTrack::read(read, Limits::for_file_len(len))
        })
        .inspect_err(|e| {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::error!(error = %e, "resume: the cursor file couldn't be read, so the cursor starts afresh");
            }
        })
        .ok();
    ResumedCursor {
        file_burned: (track.as_ref()).is_some_and(|t| t.last_times().burned.is_some()),
        writer: track.as_ref().map(CursorWriter::resumed),
        burned,
        decided,
    }
}

fn write(lines: &mut Lines<'_>, entry: &Entry) -> Result<(), RecoverError> {
    lines
        .write(entry, Durability::Ordered)
        .map_err(|e| recover::failed(e, "journal.jsonl"))
}

impl SessionCore {
    /// The core of a resumed take (the module doc's step 4): Recording, or Paused when the rebuilt clock is; runs
    /// numbered from `from.next_run`; each source's first sample notes its gap from `from.ended_at`.
    pub fn resumed(cfg: SessionConfig, mics: Vec<DeviceInfo>, from: Resumed, now_ns: u64) -> Self {
        let mut core = Self::new(cfg, mics, now_ns);
        let paused = from.clock.paused();
        core.clock = from.clock;
        core.rules_clock.start(now_ns);
        if paused {
            core.rules_clock.pause(now_ns);
            core.pause = Some(super::core::PauseWhy::Person);
        }
        core.state = if paused {
            WorkerState::Paused
        } else {
            WorkerState::Recording
        };
        core.read_back_at = Some(core.clock.elapsed_us(now_ns) + crate::readback::AFTER_US);
        core.earlier = from.done;
        core.markers = from.markers;
        core.resume_cursor(from.cursor);
        for src in &mut core.srcs {
            src.next_run = from.next_run.get(&src.id).copied().unwrap_or(1);
            src.gap_from = from.ended_at.get(&src.id).copied();
            src.status = Status::Starting;
        }
        let mics = core.mics.clone();
        for src in &mut core.srcs {
            src.rules_for_start(&mics);
        }
        core
    }
}
