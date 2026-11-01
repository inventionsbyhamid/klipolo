//! The one export slot (a single export runs at a time) and what the run on it has reported: its job, the files it
//! put in place, and whether it has ended. Start and Resume wait briefly for a run that is winding down.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use klipolo_engine::api::ExportEvent;
use klipolo_model::coded::{CodedError, Said};
use tokio::sync::watch;

#[derive(Default)]
pub struct ExportState {
    /// Taken from the moment Export (or Resume) is pressed, so Cancel works while the latest edits are saved, until the
    /// run has ended (a cancelled run's files are gone by then, so a new batch can't reuse their names).
    running: Mutex<Option<Slot>>,
    /// The files the last batch put in place, by index: Open and Show in Finder accept only these.
    files: Mutex<Vec<PathBuf>>,
    /// The folder the person picked for exports (Change); None: `Movies / Klipolo`.
    folder: Mutex<Option<PathBuf>>,
    /// Where the folders exports were written in are remembered across launches (`export-folders.json` in the app's
    /// data folder), so a kept job in a folder picked before is still listed. None: they aren't (tests).
    kept: Mutex<Option<PathBuf>>,
    /// The quit guard is on screen.
    asking: AtomicBool,
    /// The person chose Quit: the next exit request goes through.
    quitting: AtomicBool,
    /// Told when the slot is taken (true) and released (false): editing copies and thumbnail scans wait for exports.
    busy: Mutex<Vec<Busy>>,
}

/// What hears the slot taken (`true`) and released (`false`).
pub type Busy = Arc<dyn Fn(bool) + Send + Sync>;

struct Slot {
    /// `true` stops the run and deletes its job (Cancel export). Dropped when the slot is released.
    stop: watch::Sender<bool>,
    /// The job, once the run has one (the job resumed, one written by a start, `set_job`, or `ExportEvent::Started`):
    /// Quit pauses it.
    job: Option<String>,
    /// A pause was asked of it: it stops at its next part, and a Resume of the same job waits for that.
    pausing: bool,
    /// The run has said how it ended (paused, failed, finished or stopped) and is letting go of the slot.
    ended: bool,
    /// The project it exports (id, name), once known: a Cancel that doesn't name the job acts only on the open
    /// project's export.
    project: Option<(String, String)>,
}

/// Another export holds the slot.
pub(super) const BUSY: &str = "exports.refused.busy";
/// The job named isn't the one running (or none is).
const NOT_RUNNING: &str = "exports.refused.notRunning";
/// How long Start or Resume waits for a run that is winding down: one that has just said how it ended, or (Resume) the
/// same job finishing its current part after a pause.
const WIND_DOWN: Duration = Duration::from_secs(30);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ExportState {
    pub fn is_running(&self) -> bool {
        lock(&self.running).is_some()
    }

    /// Adds `hook`: `hook(true)` whenever an export takes the slot, `hook(false)` when it lets go, beside the hooks
    /// given before.
    pub fn on_busy(&self, hook: Busy) {
        lock(&self.busy).push(hook);
    }

    fn tell_busy(&self, busy: bool) {
        let hooks = lock(&self.busy).clone();
        for hook in hooks {
            hook(busy);
        }
    }

    /// The project the run on the slot exports.
    pub fn set_project(&self, id: String, name: String) {
        if let Some(slot) = lock(&self.running).as_mut() {
            slot.project = Some((id, name));
        }
    }

    /// The run's job, as soon as it is written: Quit then pauses it, even in the moment before its run starts
    /// (`pause_for_quit`), rather than stopping it.
    pub fn set_job(&self, job: String) {
        if let Some(slot) = lock(&self.running).as_mut() {
            slot.job = Some(job);
        }
    }

    /// Takes the one export slot for `job` (the job a Resume continues; None for a new export); the receiver hears
    /// Cancel. The last batch's files are forgotten.
    pub fn reserve(&self, job: Option<String>) -> Result<watch::Receiver<bool>, Said> {
        let mut running = lock(&self.running);
        if running.is_some() {
            return Err(Said::code(BUSY));
        }
        let (stop, cancel) = watch::channel(false);
        *running = Some(Slot {
            stop,
            job,
            pausing: false,
            ended: false,
            project: None,
        });
        drop(running);
        lock(&self.files).clear();
        self.tell_busy(true);
        Ok(cancel)
    }

    /// `reserve`, after waiting (briefly) for a run that is winding down, so Try again or Resume pressed the moment a
    /// failure or a pause lands, or Resume on a job still pausing (`job`), doesn't meet "An export is already
    /// running.". Another running export still does.
    pub async fn take_slot(&self, job: Option<String>) -> Result<watch::Receiver<bool>, Said> {
        let ending = {
            let running = lock(&self.running);
            match running.as_ref() {
                None => None,
                Some(slot) if slot.ended || (slot.pausing && job.is_some() && slot.job == job) => {
                    Some(slot.stop.subscribe())
                }
                Some(_) => return Err(Said::code(BUSY)),
            }
        };
        if let Some(mut ending) = ending {
            // The sender drops when the run releases the slot.
            let released = async { while ending.changed().await.is_ok() {} };
            let _ = tokio::time::timeout(WIND_DOWN, released).await;
        }
        self.reserve(job)
    }

    pub fn release(&self) {
        let released = lock(&self.running).take();
        if let Some(slot) = released {
            // A pause Quit queued for a run that never started goes with its slot: a later Resume of that job in this
            // process runs as asked.
            if let Some(job) = &slot.job {
                klipolo_engine::export::runner::forget_pause(job);
            }
            self.tell_busy(false);
        }
    }

    /// Cancel export, from the dialog or (later) a chat that may not be the one that started it: a Cancel naming `job`
    /// stops only that job; one that doesn't name it stops only the open project's (`open`, its id) export. The project
    /// is recorded as the slot is taken, so a Cancel sent while an export still saves its edits is judged too. Another
    /// project's export is refused by name, never stopped silently. Nothing running: nothing to stop.
    pub fn cancel_for(&self, job: Option<&str>, open: Option<&str>) -> Result<(), Said> {
        let running = lock(&self.running);
        let Some(slot) = running.as_ref() else {
            return Ok(());
        };
        let ours = match (job, &slot.project) {
            (Some(job), _) => slot.job.as_deref() == Some(job),
            (None, None) => true,
            (None, Some((id, _))) => open == Some(id.as_str()),
        };
        if !ours {
            return Err(match (job, &slot.project) {
                (None, Some((_, name))) => Said::Coded(
                    CodedError::new("exports.refused.otherProject").with("project", name.as_str()),
                ),
                _ => Said::code(NOT_RUNNING),
            });
        }
        let _ = slot.stop.send(true);
        Ok(())
    }

    /// Pauses the running job `job` (Pause on its row): it stops at its next part, keeping every finished part. Refused
    /// (`exports.refused.notRunning`) when it isn't running.
    pub fn pause(&self, job: &str) -> Result<(), Said> {
        if !klipolo_engine::export::runner::pause(job) {
            return Err(Said::code(NOT_RUNNING));
        }
        self.pausing(job);
        Ok(())
    }

    /// The slot's run, when it is `job`'s, was asked to pause.
    fn pausing(&self, job: &str) {
        if let Some(slot) = lock(&self.running).as_mut()
            && slot.job.as_deref() == Some(job)
        {
            slot.pausing = true;
        }
    }

    /// Quit's pause: the run's job is paused (`runner::pause_when_it_runs`), so its finished parts are kept and it
    /// lists as resumable next time; an export with no job written yet (still saving its latest changes) is stopped, as
    /// nothing of it is finished. None when nothing runs; else a receiver that hears the slot released.
    pub fn pause_for_quit(&self) -> Option<watch::Receiver<bool>> {
        let running = lock(&self.running);
        running.as_ref().map(|slot| {
            match &slot.job {
                Some(job) => {
                    klipolo_engine::export::runner::pause_when_it_runs(job);
                }
                None => {
                    let _ = slot.stop.send(true);
                }
            }
            slot.stop.subscribe()
        })
    }

    /// Follows the run's events: its job id, each file in place (by index), and its end.
    pub fn record(&self, event: &ExportEvent) {
        match event {
            ExportEvent::Started { job_id, .. } => self.set_job(job_id.clone()),
            ExportEvent::FileReady { index, path } => {
                let mut files = lock(&self.files);
                let index = *index as usize;
                if files.len() <= index {
                    files.resize(index + 1, PathBuf::new());
                }
                files[index] = PathBuf::from(path);
            }
            ExportEvent::Paused { .. }
            | ExportEvent::Failed { .. }
            | ExportEvent::Finished { .. }
            | ExportEvent::Cancelled => {
                if let Some(slot) = lock(&self.running).as_mut() {
                    slot.ended = true;
                }
            }
            _ => {}
        }
    }

    /// File `index` of the last batch, once it is in place.
    pub fn file(&self, index: usize) -> Option<PathBuf> {
        lock(&self.files)
            .get(index)
            .filter(|p| !p.as_os_str().is_empty())
            .cloned()
    }

    /// Every file the last batch put in place, in order.
    pub fn files(&self) -> Vec<PathBuf> {
        lock(&self.files)
            .iter()
            .filter(|p| !p.as_os_str().is_empty())
            .cloned()
            .collect()
    }

    /// The folder the person picked for exports, if any.
    pub fn folder(&self) -> Option<PathBuf> {
        lock(&self.folder).clone()
    }

    pub fn set_folder(&self, folder: PathBuf) {
        *lock(&self.folder) = Some(folder);
    }

    /// Remembers the folders exports were written in, in `file`, across launches.
    pub fn remember_in(&self, file: PathBuf) {
        *lock(&self.kept) = Some(file);
    }

    /// The folders exports were written in before, as remembered (none when nothing is, or the file can't be read).
    pub fn kept_folders(&self) -> Vec<PathBuf> {
        lock(&self.kept)
            .as_deref()
            .map(read_folders)
            .unwrap_or_default()
    }

    /// Remembers `dir` as a folder an export's job was written in (once).
    pub fn remember_folder(&self, dir: &Path) {
        let kept = lock(&self.kept);
        let Some(file) = kept.as_deref() else {
            return;
        };
        let mut folders = read_folders(file);
        if !folders.iter().any(|f| f == dir) {
            folders.push(dir.to_path_buf());
            write_folders(file, &folders);
        }
    }

    /// Forgets the remembered folders that hold no job any more (or are gone).
    pub fn forget_folders_without_jobs(&self) {
        let kept = lock(&self.kept);
        let Some(file) = kept.as_deref() else {
            return;
        };
        let folders = read_folders(file);
        let holding: Vec<PathBuf> = folders.iter().filter(|f| holds_jobs(f)).cloned().collect();
        if holding.len() != folders.len() {
            write_folders(file, &holding);
        }
    }

    /// Whether the quit guard is on screen; `asking(true)` returns whether it already was.
    pub fn asking(&self, now: bool) -> bool {
        self.asking.swap(now, Ordering::SeqCst)
    }

    /// The person chose Quit while an export ran (or not yet: false).
    pub fn quitting(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }

    pub fn set_quitting(&self) {
        self.quitting.store(true, Ordering::SeqCst);
    }
}

/// The folders remembered in `file`; none when it can't be read.
fn read_folders(file: &Path) -> Vec<PathBuf> {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Writes `folders` to `file`, atomically; a failure is logged (the folders are then looked in only this session).
fn write_folders(file: &Path, folders: &[PathBuf]) {
    let written = serde_json::to_vec(folders)
        .map_err(std::io::Error::other)
        .and_then(|json| {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            klipolo_platform::durable::write_atomic(
                file,
                &json,
                klipolo_platform::durable::Durability::Ordered,
            )
        });
    if let Err(error) = written {
        tracing::warn!(%error, "export: the export folders couldn't be remembered");
    }
}

/// Whether the export folder `dir` holds a job (a folder in its `.jobs` named by a job id).
fn holds_jobs(dir: &Path) -> bool {
    std::fs::read_dir(klipolo_engine::export::jobs::jobs_root(dir))
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry.path().is_dir()
                && klipolo_engine::export::jobs::valid_job_id(&entry.file_name().to_string_lossy())
        })
}

/// Lets go of the slot when dropped. The run's task holds one, so a panic in it (the runner, an event's `emit`) still
/// releases the slot; `release` is idempotent.
pub struct Releases<'a>(pub &'a ExportState);

impl Drop for Releases<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Cancel that doesn't name the job (a reloaded window, a chat command) never stops another project's export: it
    /// is refused by that project's name. Named by its job, or from its own project, it stops.
    #[test]
    fn a_stop_without_a_job_never_stops_another_projects_export() {
        let state = ExportState::default();
        let stop = state.reserve(None).unwrap();
        assert!(state.is_running());
        state.set_project("a".into(), "Launch".into());
        let refused = state.cancel_for(None, Some("b")).unwrap_err();
        assert_eq!(
            refused,
            Said::Coded(CodedError::new("exports.refused.otherProject").with("project", "Launch")),
            "refused by name, as a code"
        );
        assert!(!*stop.borrow(), "it runs on");
        state.record(&ExportEvent::Started {
            job_id: "j1".into(),
            files: Vec::new(),
            total_frames: 1,
            segments_total: 1,
        });
        assert_eq!(
            state.cancel_for(Some("j2"), Some("a")),
            Err(Said::code("exports.refused.notRunning"))
        );
        assert!(!*stop.borrow());
        state.cancel_for(Some("j1"), Some("b")).unwrap();
        assert!(*stop.borrow(), "named by its job");
        state.release();
        assert!(!state.is_running());
        // Still saving (no job yet), its project recorded as it took the slot. A Cancel from another project is
        // refused; its own project's Cancel stops it.
        let stop = state.reserve(None).unwrap();
        state.set_project("a".into(), "Launch".into());
        assert!(state.cancel_for(None, Some("b")).is_err());
        assert!(!*stop.borrow());
        state.cancel_for(None, Some("a")).unwrap();
        assert!(*stop.borrow());
        // Nothing running: nothing to stop.
        state.release();
        assert_eq!(state.cancel_for(None, Some("a")), Ok(()));
    }

    /// A run that panics still lets go of the slot, so the next export isn't refused as busy.
    #[test]
    fn a_run_that_panics_still_releases_the_slot() {
        let state = ExportState::default();
        let _stop = state.reserve(None).unwrap();
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _slot = Releases(&state);
            panic!("the export's run panicked");
        }));
        assert!(run.is_err());
        assert!(!state.is_running());
        assert!(state.reserve(None).is_ok());
    }

    #[test]
    fn a_start_waits_for_a_run_winding_down_but_not_for_another_export() {
        runtime().block_on(async {
            let state = std::sync::Arc::new(ExportState::default());
            state.reserve(None).unwrap();
            // Another export is running: no wait.
            assert_eq!(state.take_slot(None).await.unwrap_err(), Said::code(BUSY));
            // A run that has said how it ended: the start waits for the slot, then takes it.
            state.record(&ExportEvent::Failed {
                message: Said::code("exports.diskFull"),
            });
            let ending = state.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                ending.release();
            });
            state.take_slot(None).await.unwrap();
            assert!(state.is_running());
        });
    }

    /// Resume pressed on a job still pausing (its run finishing its current part) waits for it; the same job still
    /// running (Resume pressed twice at once, or from two places) and another job's export are refused at once, as busy.
    #[test]
    fn resume_waits_for_its_own_job_pausing_but_never_runs_one_twice() {
        runtime().block_on(async {
            let state = std::sync::Arc::new(ExportState::default());
            state.reserve(Some("a".into())).unwrap();
            assert_eq!(
                state.take_slot(Some("b".into())).await.unwrap_err(),
                Said::code(BUSY),
                "another job's export runs"
            );
            assert_eq!(
                state.take_slot(Some("a".into())).await.unwrap_err(),
                Said::code(BUSY),
                "the same job runs already"
            );
            state.pausing("a");
            let ending = state.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                ending.release();
            });
            state.take_slot(Some("a".into())).await.unwrap();
            assert!(state.is_running(), "the same job, once it let go");
        });
    }

    /// Quit pauses the run on the slot: one with a job is paused (its finished parts kept, resumable next time), even
    /// in the moment before its run starts; one still saving its latest changes has nothing finished, so it is stopped.
    /// The receiver hears the slot released.
    #[test]
    fn quit_pauses_a_run_with_a_job_and_stops_one_still_saving() {
        let state = ExportState::default();
        assert!(state.pause_for_quit().is_none(), "nothing runs");
        let stop = state.reserve(None).unwrap();
        let released = state.pause_for_quit().expect("it runs");
        assert!(*stop.borrow(), "no job yet: stopped");
        state.release();
        assert!(released.has_changed().is_err(), "released");
        let job = "0b9e6f3c-6d2a-4f3e-9c1b-2a7d5e8f9a10";
        let stop = state.reserve(Some(job.into())).unwrap();
        assert!(state.pause_for_quit().is_some());
        assert!(!*stop.borrow(), "a job is paused, never stopped");
        // The pause waits for the job's run; the slot let go first, it goes with it (`release`).
        state.release();
    }

    /// Editing copies (and the thumbnail scans they hold) hear the slot taken and released, each hook once, though the
    /// run lets go of it twice (its `Releases` and the release at its end).
    #[test]
    fn every_hook_hears_the_slot_taken_and_released() {
        let state = ExportState::default();
        let heard = Arc::new(Mutex::new(Vec::new()));
        for who in ["copies", "other"] {
            let heard = heard.clone();
            state.on_busy(Arc::new(move |busy| {
                heard.lock().unwrap().push((who, busy))
            }));
        }
        let _stop = state.reserve(None).unwrap();
        state.release();
        state.release();
        assert_eq!(
            *heard.lock().unwrap(),
            [
                ("copies", true),
                ("other", true),
                ("copies", false),
                ("other", false)
            ]
        );
    }

    /// Open and Show in Finder reach only the files the last batch put in place, by index; a new batch forgets them.
    #[test]
    fn only_the_last_batchs_files_are_known() {
        let state = ExportState::default();
        let _stop = state.reserve(None).unwrap();
        state.record(&ExportEvent::FileReady {
            index: 1,
            path: "/m/Demo 9x16.mp4".into(),
        });
        assert_eq!(state.file(0), None, "not in place yet");
        assert_eq!(state.file(1), Some(PathBuf::from("/m/Demo 9x16.mp4")));
        assert_eq!(state.files(), [PathBuf::from("/m/Demo 9x16.mp4")]);
        state.record(&ExportEvent::Finished { notes: None });
        state.release();
        let _stop = state.reserve(None).unwrap();
        assert_eq!(state.file(1), None);
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
    }
}
