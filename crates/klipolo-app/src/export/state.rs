//! The one export slot (a single export runs at a time) and what the run on it has reported: its job, the files it
//! put in place, and whether it has ended. A start waits briefly for a run that is winding down.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use klipolo_engine::api::ExportEvent;
use klipolo_model::coded::{CodedError, Said};
use tokio::sync::watch;

#[derive(Default)]
pub struct ExportState {
    /// Taken from the moment Export is pressed, so Cancel works while the latest edits are saved, until the run has
    /// ended (a cancelled run's files are gone by then, so a new batch can't reuse their names).
    running: Mutex<Option<Slot>>,
    /// The files the last batch put in place, by index: Open and Show in Finder accept only these.
    files: Mutex<Vec<PathBuf>>,
    /// The folder the person picked for exports (Change); None: `Movies / Klipolo`.
    folder: Mutex<Option<PathBuf>>,
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
    /// The job, once the run has one (written by a start, `set_job`, or `ExportEvent::Started`).
    job: Option<String>,
    /// The job's folder, once written (`set_job_dir`): quitting deletes it on the spot (`cleanup::clear_on_quit`).
    job_dir: Option<PathBuf>,
    /// The run has said how it ended (failed, finished or stopped) and is letting go of the slot.
    ended: bool,
    /// The project it exports (id, name), once known: a Cancel that doesn't name the job acts only on the open
    /// project's export.
    project: Option<(String, String)>,
}

/// Another export holds the slot.
pub(super) const BUSY: &str = "exports.refused.busy";
/// The job named isn't the one running (or none is).
const NOT_RUNNING: &str = "exports.refused.notRunning";
/// How long a start waits for a run that has just said how it ended.
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

    /// The run's job, as soon as it is written.
    pub fn set_job(&self, job: String) {
        if let Some(slot) = lock(&self.running).as_mut() {
            slot.job = Some(job);
        }
    }

    /// The run's job folder, as soon as it is written.
    pub fn set_job_dir(&self, dir: PathBuf) {
        if let Some(slot) = lock(&self.running).as_mut() {
            slot.job_dir = Some(dir);
        }
    }

    /// Takes the one export slot; the receiver hears Cancel. The last batch's files are forgotten.
    pub fn reserve(&self) -> Result<watch::Receiver<bool>, Said> {
        let mut running = lock(&self.running);
        if running.is_some() {
            return Err(Said::code(BUSY));
        }
        let (stop, cancel) = watch::channel(false);
        *running = Some(Slot {
            stop,
            job: None,
            job_dir: None,
            ended: false,
            project: None,
        });
        drop(running);
        lock(&self.files).clear();
        self.tell_busy(true);
        Ok(cancel)
    }

    /// `reserve`, after waiting (briefly) for a run that has just said how it ended, so Try again pressed the moment a
    /// failure lands doesn't meet "An export is already running.". Another running export still does.
    pub async fn take_slot(&self) -> Result<watch::Receiver<bool>, Said> {
        let ending = {
            let running = lock(&self.running);
            match running.as_ref() {
                None => None,
                Some(slot) if slot.ended => Some(slot.stop.subscribe()),
                Some(_) => return Err(Said::code(BUSY)),
            }
        };
        if let Some(mut ending) = ending {
            // The sender drops when the run releases the slot.
            let released = async { while ending.changed().await.is_ok() {} };
            let _ = tokio::time::timeout(WIND_DOWN, released).await;
        }
        self.reserve()
    }

    pub fn release(&self) {
        let released = lock(&self.running).take();
        if released.is_some() {
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

    /// Quit's stop: the run is stopped, as Cancel does. None when nothing runs; else its job and folder as far as they
    /// are written, and a receiver that hears the slot released.
    pub fn stop_for_quit(&self) -> Option<Stopped> {
        let running = lock(&self.running);
        running.as_ref().map(|slot| {
            let _ = slot.stop.send(true);
            Stopped {
                job: slot.job.clone(),
                dir: slot.job_dir.clone(),
                released: slot.stop.subscribe(),
            }
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

/// A run Quit stopped: its job and folder (when written), and what hears its slot released.
pub struct Stopped {
    pub job: Option<String>,
    pub dir: Option<PathBuf>,
    pub released: watch::Receiver<bool>,
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
        let stop = state.reserve().unwrap();
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
        let stop = state.reserve().unwrap();
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
        let _stop = state.reserve().unwrap();
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _slot = Releases(&state);
            panic!("the export's run panicked");
        }));
        assert!(run.is_err());
        assert!(!state.is_running());
        assert!(state.reserve().is_ok());
    }

    #[test]
    fn a_start_waits_for_a_run_winding_down_but_not_for_another_export() {
        runtime().block_on(async {
            let state = std::sync::Arc::new(ExportState::default());
            state.reserve().unwrap();
            // Another export is running: no wait.
            assert_eq!(state.take_slot().await.unwrap_err(), Said::code(BUSY));
            // A run that has said how it ended: the start waits for the slot, then takes it.
            state.record(&ExportEvent::Failed {
                message: Said::code("exports.diskFull"),
            });
            let ending = state.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                ending.release();
            });
            state.take_slot().await.unwrap();
            assert!(state.is_running());
        });
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
        let _stop = state.reserve().unwrap();
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
        let _stop = state.reserve().unwrap();
        state.record(&ExportEvent::FileReady {
            index: 1,
            path: "/m/Demo 9x16.mp4".into(),
        });
        assert_eq!(state.file(0), None, "not in place yet");
        assert_eq!(state.file(1), Some(PathBuf::from("/m/Demo 9x16.mp4")));
        assert_eq!(state.files(), [PathBuf::from("/m/Demo 9x16.mp4")]);
        state.record(&ExportEvent::Finished { notes: None });
        state.release();
        let _stop = state.reserve().unwrap();
        assert_eq!(state.file(1), None);
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
    }
}
