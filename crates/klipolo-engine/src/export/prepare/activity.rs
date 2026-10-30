//! Export's screen activity before its first frame: the screen files whose
//! activity its plans read (`screen_files`: those of the takes under an Auto zoom or following, as `wanted` names them
//! for the preview too) have every chunk made first, so export's camera plans and follow paths are the preview's.
//!
//! - **Made once.** The files are the editor's own cache (`ExportJob::activity_dir`): what the editor or an earlier
//!   run analysed is read, never decoded again; the rest is made now (`ActivityStore::ensure`) off the runtime's
//!   threads, telling the parent how far it is (`WorkerLine::Analysing`, the spec's "Analysing the screen…").
//! - **Stoppable.** A pause or a stop ends it within a tick (the decoder checks between packets), keeping every whole
//!   minute made: a chunk is appended whole and synced, so nothing half-made is kept, and the job resumes from there.
//! - **Never plans differently from the editor without saying so.** A minute whose pictures can't be read is passed
//!   over and planned without, as the editor's pass does. A file that can't be analysed, for good (no
//!   video, no length, no decoder, a container FFmpeg rejects, no picture at 0), is planned without its activity and
//!   marked so in the shared folder (`ActivityStore::fail`), so the editor plans without it too, and so does every later
//!   run of this job. One that can't be read now (an OS error, the file not there, another pass holding it) is tried
//!   again (`RETRY_WAITS`), then the export asks, naming it (`unreadable`): Try again, or export without it; it is never
//!   marked. The answer is that screen's (`ExportJob::activity_without`): another that can't
//!   be read asks too, and one the person said to export without is tried once, then planned with the minutes already
//!   made, as the editor plans with them. A full disk stops the run (it pauses, as
//!   it would for its own files).
//!
//! **Cost:** a file analysed already is one read of its activity file (≤ 0.5 MB an hour: about a millisecond for two
//! hours); the rest is the pass's. **Memory:** the tracks (14 B a tick, ≤ 0.5 MB an hour), one decoder
//! at a time.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use klipolo_cursor::activity::ActivityTrack;
use klipolo_model::coded::{CodedError, ErrorParam};
use klipolo_model::project::Project;
use klipolo_model::time::Us;

use crate::api::EXPORT_ACTIVITY_UNREADABLE;
use crate::export::protocol::{ErrorCode, Lines, WorkerLine};
use crate::export::shared::{Control, WorkerError, disk_full, internal};
use crate::export::worker::send;
use crate::media::activity::ActivityStore;
use crate::media::cursor::{ActivityOf, Want};
use crate::platform::activity::ActivityError;

/// How often a pause or a stop is looked for while the screens are analysed.
const STOP_POLL: Duration = Duration::from_millis(50);
/// The parent hears from the worker at least this often meanwhile (a minute of 4K may take longer to analyse).
const BEAT: Duration = Duration::from_secs(5);
/// The waits before each further try of a screen that can't be read now: three more tries, 15 s
/// of waiting in all (each try also waits up to `ensure`'s second or two for another pass to let the file go).
pub const RETRY_WAITS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(4),
    Duration::from_secs(10),
];

/// What `analyse` does with a screen that can't be read now (an OS error, the file not there, another pass holding
/// it): it tries again after each of `waits`, then asks (`Analysis::Unreadable`), unless the person said to export
/// without that screen (its path in `without`, `ExportJob::activity_without`): then it is tried once and planned with
/// the minutes already made (`ActivityStore::made_so_far`), never marked.
#[derive(Clone)]
pub struct Retry<'a> {
    pub waits: &'a [Duration],
    pub without: Vec<String>,
}

/// The screens' activity export plans with: each file's track as made before frame 1; none for a file that can't be
/// analysed. By path.
#[derive(Clone, Default)]
pub struct Analysed(HashMap<String, Arc<ActivityTrack>>);

impl Analysed {
    /// `path`'s track, if it was made.
    pub fn get(&self, path: &str) -> Option<Arc<ActivityTrack>> {
        self.0.get(path).cloned()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Where export's takes read it (`CursorTracks::set_activity`).
    pub fn source(self) -> ActivityOf {
        let tracks = Arc::new(self);
        Arc::new(move |path: &str| tracks.get(path))
    }
}

/// How an analysis ended.
pub enum Analysis {
    Done(Analysed),
    /// Paused or stopped: every whole minute made is kept.
    Stopped,
    /// This screen can't be read now, after every try (`Retry`): the export asks, naming it (`unreadable`).
    Unreadable(String),
}

/// The question an export asks when the screen at `path` can't be read now (`ErrorCode::ActivityUnreadable`, the
/// file's name as `file`): Retry, or export without its activity. Not retried by the runner: it waits for the answer.
pub fn unreadable(path: &str) -> WorkerError {
    let name = ErrorParam::Text(crate::log_name(path).into_owned());
    let said = CodedError::new(EXPORT_ACTIVITY_UNREADABLE).with("file", name);
    WorkerError {
        code: ErrorCode::ActivityUnreadable,
        message: serde_json::to_string(&said).unwrap_or_default(),
        retryable: false,
    }
}

/// The screen files export's plans read activity from: the screens of `wants` (`p`'s `wanted`) whose plans read them
/// (`Want::plans_read_activity`: under an Auto zoom, or following), each once, in timeline order, with its length
/// (from the project). O(wanted × files + assets).
pub fn screen_files(p: &Project, wants: &[Want]) -> Vec<(String, Us)> {
    let lengths: HashMap<&str, Us> = (p.assets.iter())
        .map(|a| (a.path.as_str(), a.duration_us))
        .collect();
    let mut seen = HashSet::new();
    (wants.iter())
        .filter(|w| w.plans_read_activity)
        .flat_map(|w| w.screens.iter().cloned())
        .filter(|(_, path)| seen.insert(path.clone()))
        .map(|(_, path)| {
            let len = lengths.get(path.as_str()).copied().unwrap_or(0);
            (path, len)
        })
        .collect()
}

/// Blocking: every chunk of `files` made in `store`, in order (see the module notes), with `progress` after each minute
/// made: the share done of them all, by length. Stopped once `cancel` is set. Fails only on a full disk.
pub fn analyse(
    store: &ActivityStore,
    files: &[(String, Us)],
    cancel: &Arc<AtomicBool>,
    progress: &dyn Fn(f32),
    retry: Retry,
) -> Result<Analysis, ActivityError> {
    let weight = |len: Us| len.max(1) as f64;
    let total: f64 = files.iter().map(|&(_, len)| weight(len)).sum();
    let mut before = 0.0;
    let mut made = HashMap::with_capacity(files.len());
    for (path, len) in files {
        if cancel.load(Ordering::SeqCst) {
            return Ok(Analysis::Stopped);
        }
        let share = |of_file: f32| ((before + f64::from(of_file) * weight(*len)) / total) as f32;
        if !store.is_analysed(path) {
            progress(share(0.0));
        }
        let without = retry.without.contains(path);
        // Once the person has answered, one try is enough.
        let mut waits = if without { &[][..] } else { retry.waits }.iter();
        let track = loop {
            match store.ensure(path, cancel, &|of_file| progress(share(of_file))) {
                Ok(track) => break Some(track),
                Err(ActivityError::Cancelled) => return Ok(Analysis::Stopped),
                Err(ActivityError::DiskFull) => return Err(ActivityError::DiskFull),
                Err(ActivityError::Unsupported(error)) => {
                    tracing::warn!(file = %crate::log_name(path), %error, "export: a screen can't be analysed; planned without its activity");
                    store.fail(path);
                    break None;
                }
                Err(error) => match waits.next() {
                    Some(&wait) => {
                        tracing::info!(file = %crate::log_name(path), %error, ?wait, "export: a screen can't be read now; trying again");
                        if !rest(wait, cancel) {
                            return Ok(Analysis::Stopped);
                        }
                    }
                    // Planned as the editor plans it: with the minutes already made.
                    None if without => {
                        tracing::warn!(file = %crate::log_name(path), %error, "export: a screen can't be read now; planned with the activity made so far, as the person said");
                        break store.made_so_far(path);
                    }
                    None => {
                        tracing::warn!(file = %crate::log_name(path), %error, "export: a screen can't be read now; asking");
                        return Ok(Analysis::Unreadable(path.clone()));
                    }
                },
            }
        };
        if let Some(track) = track {
            made.insert(path.clone(), track);
        }
        before += weight(*len);
    }
    Ok(Analysis::Done(Analysed(made)))
}

/// Waits `wait`, or less when `cancel` is set (false then).
fn rest(wait: Duration, cancel: &AtomicBool) -> bool {
    let until = Instant::now() + wait;
    while !cancel.load(Ordering::SeqCst) {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(STOP_POLL));
    }
    false
}

/// Before the first frame of a run that draws frames: `analyse` of `files` in `store` (None: no activity folder, so
/// the plans go without) on a blocking thread, each minute made said as `WorkerLine::Analysing` (and every few seconds
/// meanwhile), a pause or a stop passed on within `STOP_POLL`. None when it was paused or stopped. A screen it asks
/// about is told to `asked` first (its path), so the person's answer is that screen's.
pub async fn analyse_screens<W: std::io::Write>(
    store: Option<Arc<ActivityStore>>,
    files: Vec<(String, Us)>,
    out: &mut Lines<W>,
    control: &Control,
    retry: Retry<'static>,
    asked: &(dyn Fn(&str) + Sync),
) -> Result<Option<Analysed>, WorkerError> {
    let Some(store) = store.filter(|_| !files.is_empty()) else {
        return Ok(Some(Analysed::default()));
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, mut said) = tokio::sync::mpsc::unbounded_channel::<f32>();
    let task = {
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            let progress = |done| {
                let _ = tx.send(done);
            };
            analyse(&store, &files, &cancel, &progress, retry)
        })
    };
    tokio::pin!(task);
    let mut poll = tokio::time::interval(STOP_POLL);
    let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + BEAT, BEAT);
    let mut last: Option<f32> = None;
    let ended = loop {
        tokio::select! {
            ended = &mut task => break ended.map_err(internal)?,
            Some(done) = said.recv() => {
                last = Some(done);
                send(out, &WorkerLine::Analysing { done }, control);
            }
            _ = poll.tick() => {
                if !control.running() {
                    cancel.store(true, Ordering::SeqCst);
                }
            }
            _ = beat.tick() => match last {
                Some(done) => send(out, &WorkerLine::Analysing { done }, control),
                None => send(out, &WorkerLine::Preparing, control),
            },
        }
    };
    while let Ok(done) = said.try_recv() {
        last = Some(done);
    }
    match ended {
        Ok(Analysis::Done(analysed)) => {
            if last.is_some() {
                send(out, &WorkerLine::Analysing { done: 1.0 }, control);
            }
            Ok(Some(analysed))
        }
        Ok(Analysis::Stopped) => Ok(None),
        Ok(Analysis::Unreadable(path)) => {
            asked(&path);
            Err(unreadable(&path))
        }
        Err(ActivityError::DiskFull) => Err(disk_full()),
        Err(error) => Err(internal(error)),
    }
}
