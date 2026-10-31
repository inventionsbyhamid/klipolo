//! `scan`: the takes in the recordings folder and what state each is in, cheaply enough for Home's card to show
//! within 2 s of launch.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use klipolo_model::project::{RecordingKind, SourceRole};

use crate::journal::Entry;
use crate::session::{self, EXTENSION, SessionDir, SessionLock};

/// The most takes `scan` lists.
pub const SCAN_LIMIT: usize = 500;
/// How much of the journal's end `scan` reads: `Finalized` and `Imported` are its last lines, and a 3-hour journal
/// is about 2 MB. It reads `FIRST_TAIL_BYTES` first, and all of this only when that doesn't say how the take ended.
const TAIL_BYTES: u64 = 64 * 1024;
const FIRST_TAIL_BYTES: u64 = 4 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoundState {
    /// A recorder holds the take's lock (it may be orphaned: the editor closed, the worker records on).
    Live,
    /// The recorder stopped without finishing: repair it, then import it.
    Interrupted,
    /// Stop finished it, but it was never imported.
    Ready,
    Imported,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The take's folder.
    pub dir: PathBuf,
    pub id: String,
    pub state: FoundState,
    /// When it was set up, ms since 1970 (`SessionConfig::created_ms`).
    pub started_ms: u64,
    pub kind: RecordingKind,
    /// Its sources' roles, in its config's order.
    pub roles: Vec<SourceRole>,
}

/// Every `*.klipolorec` under `root` with a readable `session.json`, newest first by `created_ms`, at most `limit`. Per
/// take it reads the config, takes the lock for an instant (held: `Live`; the probe never retries, so a recorder that
/// starts at that instant retries instead) and reads the journal's last 64 KB: `Imported`, else `Finalized`
/// (`Ready`), else `Interrupted`. A missing root lists nothing.
pub fn scan(root: &Path, limit: usize) -> Vec<Found> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => {
            if e.kind() != io::ErrorKind::NotFound {
                tracing::warn!(error = %e, "recovery: can't list the recordings folder");
            }
            return Vec::new();
        }
    };
    let mut takes: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == EXTENSION) && p.is_dir())
        .filter_map(|p| {
            let dir = SessionDir::at(&p);
            // Imported: skipped by its marker alone, before its config, its lock or its journal.
            if dir.imported_mark().exists() {
                return None;
            }
            match session::read_config(&dir) {
                Ok(cfg) => Some((dir, cfg)),
                Err(e) => {
                    tracing::warn!(error = %e, "recovery: a take without a readable session.json is skipped");
                    None
                }
            }
        })
        .collect();
    takes.sort_by(|(_, a), (_, b)| {
        b.created_ms
            .cmp(&a.created_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    takes.truncate(limit);
    takes
        .into_iter()
        .map(|(dir, cfg)| Found {
            state: state_of(&dir),
            dir: dir.root,
            id: cfg.id,
            started_ms: cfg.created_ms,
            kind: cfg.kind,
            roles: cfg.sources.iter().map(|s| s.role).collect(),
        })
        .collect()
}

/// One take's state, as `scan` lists it: its lock taken for an instant (held: `Live`, at once, with no retry: a caller
/// that must own the take retries for about 500 ms first), then the journal's last 64 KB.
pub fn state_of(dir: &SessionDir) -> FoundState {
    match SessionLock::try_acquire(dir) {
        Ok(None) => return FoundState::Live,
        // Released at once.
        Ok(Some(_)) => {}
        // Can't tell (a read-only volume, say): the journal decides, and repair reports the error.
        Err(e) => tracing::warn!(error = %e, "recovery: can't probe a take's lock"),
    }
    let end = ending(dir);
    if end.imported.is_some() {
        FoundState::Imported
    } else if end.finalized {
        FoundState::Ready
    } else {
        FoundState::Interrupted
    }
}

/// How a take's journal says it ended, from its last 64 KB (`scan`'s read; the app's resume rule).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ending {
    /// The person's Stop reached the journal.
    pub stop_requested: bool,
    pub finalized: bool,
    pub recovered: bool,
    /// The project it was imported into, last.
    pub imported: Option<String>,
    /// The project an import of it chose, last (`Entry::Importing`, written before that import changed anything).
    pub importing: Option<String>,
}

/// `dir`'s `Ending`; a journal that can't be read says nothing.
pub fn ending(dir: &SessionDir) -> Ending {
    let ended = |t: &[Entry]| {
        t.iter().any(|e| {
            matches!(
                e,
                Entry::Finalized { .. } | Entry::Recovered { .. } | Entry::Imported { .. }
            )
        })
    };
    let read = tail(&dir.journal(), FIRST_TAIL_BYTES).and_then(|first| {
        if ended(&first) {
            Ok(first)
        } else {
            tail(&dir.journal(), TAIL_BYTES)
        }
    });
    let tail = match read {
        Ok(tail) => tail,
        Err(e) => {
            if e.kind() != io::ErrorKind::NotFound {
                tracing::warn!(error = %e, "recovery: can't read a take's journal");
            }
            Vec::new()
        }
    };
    let mut end = Ending::default();
    for entry in tail {
        match entry {
            Entry::StopRequested { .. } => end.stop_requested = true,
            Entry::Finalized { .. } => end.finalized = true,
            Entry::Recovered { .. } => end.recovered = true,
            Entry::Imported { project } => end.imported = Some(project),
            Entry::Importing { project } => end.importing = Some(project),
            _ => {}
        }
    }
    end
}

/// The entries in the journal's last `bytes`. A read that starts after the first byte starts one byte early and skips
/// up to the first newline: a line cut by the start is left out, a whole one isn't. Lines that don't parse (a torn
/// last one) are ignored.
fn tail(path: &Path, bytes: u64) -> io::Result<Vec<Entry>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let from = len.saturating_sub(bytes + 1);
    file.seek(SeekFrom::Start(from))?;
    let mut read = Vec::with_capacity((len - from) as usize);
    file.take(bytes + 1).read_to_end(&mut read)?;
    let mut lines = read.split(|&b| b == b'\n');
    if from > 0 {
        lines.next();
    }
    Ok(lines
        .filter_map(|line| serde_json::from_slice(line.trim_ascii()).ok())
        .collect())
}
