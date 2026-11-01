//! Recovery's per-file step and its journal lines, shared with the resumed recorder (its crashed runs) and Stop (a run
//! the writer didn't write exactly): `finish_file`, `Lines`, a sound run's levels and the take's lock.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use klipolo_platform::durable::{DiskReason, Durability, classify};

use super::runs::RunFile;
use super::sound::{self, repair_sound};
use super::{LOCK_RETRY, LOCK_WAIT, RecoverError, RunOutcome, failed, video};
use crate::english::msg;
use crate::fail::fail_point;
use crate::io::Io;
use crate::journal::{self, Entry, JournalRead};
use crate::manifest::source_param;
use crate::session::{SessionDir, SessionLock};

/// One run file finished as repair finishes it: video by `repair_video` (its `Durable` and `Remux` lines through
/// `lines`), sound by `repair_sound`. `step(bytes)` hears how far a video file's check past its durable mark has read,
/// and stops it (an `Interrupted` error) when it returns false. Repair, a resumed recorder (its crashed runs) and Stop
/// (a run the writer didn't write exactly) all use it.
pub(crate) fn finish_file(
    io: &dyn Io,
    path: &Path,
    run: &RunFile,
    durable: Option<u64>,
    lines: &mut Lines<'_>,
    step: &mut dyn FnMut(u64) -> bool,
) -> io::Result<RunOutcome> {
    if run.video {
        let mut line = |e: &Entry| lines.write(e, Durability::Ordered);
        video::repair_video_watched(
            io,
            path,
            &run.file,
            run.stream.as_ref(),
            durable,
            &mut line,
            step,
        )
    } else {
        repair_sound(io, path, run.format, durable)
    }
}

/// A running level from the journal: the peaks so far and the file length they cover.
pub(crate) type Running = (Vec<f32>, u64);

/// A sound run's `Entry::Levels` over its sized file: from its last running level (`from`) to the end, so only what
/// came after the last tick is read (a whole hour is never read for them); with none, the
/// whole run, which then crashed before its first tick. `step` hears the progress and can stop the read. None: the
/// file isn't a CAF of ours.
pub(crate) fn levels(
    path: &Path,
    key: &str,
    from: Option<&Running>,
    step: &mut dyn FnMut(u64) -> bool,
) -> io::Result<Option<Entry>> {
    fail_point!("recover.levels")?;
    let file = File::open(path)?;
    let Ok(scan) = crate::caf::scan(&mut &file, file.metadata()?.len()) else {
        return Ok(None);
    };
    let first = from.map_or(0, |(_, through)| {
        through.saturating_sub(scan.data_at) / scan.frame_bytes()
    });
    let mut peaks = sound::channel_peaks(&file, &scan, (first, scan.frames), step)?;
    if let Some((earlier, _)) = from {
        for (peak, was) in peaks.iter_mut().zip(earlier) {
            *peak = peak.max(*was);
        }
    }
    Ok(Some(Entry::Levels {
        file: key.to_owned(),
        peaks,
        through: None,
    }))
}

/// What a crashed sound run's levels give the journal: its `Levels`; or, when its file can't be read for them, a mild
/// note (`label`: the source's), since levels are only a hint for import. A bad sector never fails a repair or a resume.
/// `Ok(None)`: nothing to say (the file isn't a CAF of ours). Err only when `step` stopped
/// the read (a cancelled repair).
pub(crate) fn levels_or_note(
    path: &Path,
    run: &RunFile,
    label: &str,
    from: Option<&Running>,
    step: &mut dyn FnMut(u64) -> bool,
) -> io::Result<Option<Entry>> {
    let mut stopped = false;
    let mut watched = |n| {
        let go = step(n);
        stopped |= !go;
        go
    };
    match levels(path, &run.file, from, &mut watched) {
        Ok(entry) => Ok(entry),
        Err(e) if stopped => Err(e),
        Err(e) => {
            tracing::warn!(file = run.file, error = %e, "recovery: a sound run's levels couldn't be read, so it imports without them");
            let note = msg!("capture.note.levels", source = source_param(label));
            Ok(Some(Entry::health(&note, Some(run.source.clone()), true)))
        }
    }
}

/// The run files whose levels the journal already has for the whole run.
pub(crate) fn leveled(journal: &JournalRead) -> BTreeSet<String> {
    journal
        .entries
        .iter()
        .filter_map(|e| match e {
            Entry::Levels {
                file,
                through: None,
                ..
            } => Some(file.clone()),
            _ => None,
        })
        .collect()
}

/// Each run file's last running level in the journal (`Entry::Levels` with `through`).
pub(crate) fn running(journal: &JournalRead) -> BTreeMap<String, Running> {
    let mut out = BTreeMap::new();
    for e in &journal.entries {
        if let Entry::Levels {
            file,
            peaks,
            through: Some(through),
        } = e
        {
            out.insert(file.clone(), (peaks.clone(), *through));
        }
    }
    out
}

/// A disk error that stops the whole repair, not just one run: a full or read-only disk fails every run alike, and
/// freeing space lets a later repair go on.
pub(crate) fn stops_the_take(e: &io::Error) -> bool {
    matches!(classify(e), DiskReason::Full | DiskReason::ReadOnly)
}

/// The lock, retried for `LOCK_WAIT` before the take counts as live.
pub(crate) fn hold(dir: &SessionDir) -> Result<SessionLock, RecoverError> {
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match SessionLock::try_acquire(dir) {
            Ok(Some(lock)) => return Ok(lock),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(LOCK_RETRY),
            Ok(None) => return Err(RecoverError::Live),
            Err(e) => return Err(failed(e, "session.lock")),
        }
    }
}

/// Journal lines written outside the durable writer (repair, a resumed recorder before its writer starts, Stop after
/// its writer finished), through `journal::open_append` (a line a crash tore is ended first), made only when there is
/// something to write; the journal is created if it was lost. Each line is synced as asked.
pub(crate) struct Lines<'a> {
    io: &'a dyn Io,
    path: PathBuf,
    file: Option<File>,
    /// A write or sync failed: whatever failed after it may be the journal's fault, not a run's.
    pub(crate) failed: bool,
}

impl<'a> Lines<'a> {
    pub(crate) fn new(io: &'a dyn Io, path: PathBuf) -> Self {
        Self {
            io,
            path,
            file: None,
            failed: false,
        }
    }

    pub(crate) fn write(&mut self, entry: &Entry, durability: Durability) -> io::Result<()> {
        let written = self.write_once(entry, durability);
        self.failed |= written.is_err();
        written
    }

    fn write_once(&mut self, entry: &Entry, durability: Durability) -> io::Result<()> {
        let file = match &mut self.file {
            Some(file) => file,
            None => self.file.insert(self.io.open_append(&self.path)?),
        };
        self.io.append(file, &self.path, &journal::line(entry))?;
        crate::io::sync(self.io, file, &self.path, durability)
    }
}
