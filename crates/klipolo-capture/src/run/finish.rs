//! Stop's finishing steps, after the core's `Finalize` and the writer's `finish` (whose last tick journals every file's
//! final length, so recovery's durable bound never cuts what Stop made durable):
//! - **Video runs** the writer wrote exactly (`WriterSnapshot::wrote_exactly`): the soft remux planned from the run's
//!   own muxer index, so no walk is needed and an hour finishes in under a second. Any other: recovery's per-file
//!   steps, which walk the file (a lost or failed append left it shorter than its muxer).
//! - **Sound runs**: sized (`repair_sound`: their frames are all durable now).
//! - Each file is reopened read and write here, never through the writer's handles, which append (a positioned write
//!   on one lands at the end on Linux).
//! - Then the manifest, derived from the journal as recovery derives it, and `Finalized`.
//!
//! A resumed take's earlier runs were finished by `run::resume`; their lengths come in `earlier`.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

use klipolo_model::time::Us;
use klipolo_platform::durable::Durability;

use super::core::{RunDone, RunKind};
use crate::english::msg;
use crate::fmp4::finalize;
use crate::io::Io;
use crate::journal::{self, Entry};
use crate::manifest::{self, Manifest, source_param};
use crate::recover::{self, Lines, RunFile, RunOutcome};
use crate::session::{FileKey, SessionConfig, SessionDir};
use crate::writer::WriterSnapshot;

/// A take whose Stop was journaled but whose finishing a crash cut short (`Resumed::stopped`):
/// `run::resume` finished its runs (their lengths in `earlier`); this writes the manifest and `Finalized`, unless the
/// take already has a manifest and `Finalized`, or `Recovered` (recovery finished it). No pipe and no writer: nothing
/// records again.
pub fn finish_stopped(
    io: &dyn Io,
    dir: &SessionDir,
    cfg: &SessionConfig,
    earlier: &BTreeMap<FileKey, Us>,
) -> io::Result<()> {
    let journal = journal::read(&dir.journal())?;
    let finished = (journal.entries.iter())
        .any(|e| matches!(e, Entry::Finalized { .. } | Entry::Recovered { .. }));
    if finished && dir.manifest().exists() {
        return Ok(());
    }
    let snap = WriterSnapshot::default();
    finish_take(io, dir, cfg, &[], earlier, &snap, &AtomicU64::new(0)).map(|_| ())
}

/// Finishes the take (the module doc). `steps` counts each run finished, for Stop's watchdog.
pub fn finish_take(
    io: &dyn Io,
    dir: &SessionDir,
    cfg: &SessionConfig,
    runs: &[RunDone],
    earlier: &BTreeMap<FileKey, Us>,
    snap: &WriterSnapshot,
    steps: &AtomicU64,
) -> io::Result<Manifest> {
    let mut lines = Lines::new(io, dir.journal());
    let mut durations = earlier.clone();
    for r in runs {
        steps.fetch_add(1, Ordering::Relaxed);
        let path = dir.key_path(&r.key)?;
        let exact = match &r.kind {
            RunKind::Video {
                plan: Some(plan),
                bytes_written,
                duration_us,
                ..
            } if snap.wrote_exactly(&r.key, *bytes_written) => Some((plan, *duration_us)),
            _ => None,
        };
        if let Some((plan, duration_us)) = exact {
            let file = OpenOptions::new().read(true).write(true).open(&path)?;
            finalize::write_tail(io, &file, &path, plan)?;
            lines.write(
                &Entry::Remux {
                    file: r.key.clone(),
                    done: false,
                },
                Durability::Ordered,
            )?;
            finalize::flip(io, &file, &path, plan)?;
            lines.write(
                &Entry::Remux {
                    file: r.key.clone(),
                    done: true,
                },
                Durability::Ordered,
            )?;
            durations.insert(r.key.clone(), duration_us);
            continue;
        }
        let run = RunFile {
            source: r.source.clone(),
            run: r.run,
            file: r.key.clone(),
            video: matches!(r.kind, RunKind::Video { .. }),
            stream: match &r.kind {
                RunKind::Video { info, .. } => Some(info.clone()),
                RunKind::Sound { .. } => None,
            },
            format: match r.kind {
                RunKind::Sound { format } => Some(format),
                RunKind::Video { .. } => None,
            },
            offset_us: None,
        };
        let durable = snap.durable.get(&r.key).copied();
        match recover::finish_file(io, &path, &run, durable, &mut lines, &mut |_| true)? {
            RunOutcome::Repaired(done) => {
                durations.insert(r.key.clone(), done.duration_us);
            }
            RunOutcome::Empty => {}
            RunOutcome::LeftOut => {
                let label = cfg.label_of(&r.source);
                let note = msg!("capture.recovered.partKept", source = source_param(label));
                lines.write(
                    &Entry::health(&note, Some(r.source.clone()), true),
                    Durability::Ordered,
                )?;
            }
        }
    }
    let read = journal::read(&dir.journal())?;
    let anchors = journal::read_anchors(&dir.timing())?;
    let m = manifest::derive(cfg, &read, &anchors, &durations);
    manifest::write_manifest(dir, &m)?;
    lines.write(
        &Entry::Finalized {
            duration_us: m.duration_us,
        },
        Durability::Full,
    )?;
    Ok(m)
}
