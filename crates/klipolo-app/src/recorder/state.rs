//! The recorder in the app: its state (`RecorderState`, managed from launch) and what each command does with it. In a
//! test run (`KLIPOLO_TEST`, set by every test command and every test launch of the app) it holds no backend and
//! launches no worker: setup answers `capture.error.testRun` ("Recording is off in this test run.") and every other
//! command refuses with it, before any worker or OS call.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use klipolo_capture::backend::{Backend, Permission};
use klipolo_capture::preflight::RecorderSetup;
use klipolo_capture::protocol::Command;
use klipolo_capture::session::ImportTarget;
use klipolo_model::coded::{CodedError, Said};
use klipolo_platform::clock::{Clock, HostClock};
use tauri::{AppHandle, Manager};

use super::env::{Env, Sink};
use super::launch::{Launch, ProcessLaunch, WorkerIo};
use super::permissions::{self, Open};
use super::setup::{self, Files, Probing, Shown};
use super::sink::AppSink;
use super::take::Recorder;
use super::types::{ArmOptions, ArmedTake, PrivacyPage, StoppedTake};
use super::window::{MainWindow, Window};
use super::words;
use crate::project::{ProjectSessionState, with_open};

/// The variable that makes a run a test run: the capture crate's consent guard reads it too.
const TEST_GUARD: &str = "KLIPOLO_TEST";

/// Whether this process is a test run (`KLIPOLO_TEST` is set).
pub(super) fn test_run() -> bool {
    std::env::var_os(TEST_GUARD).is_some()
}

/// What the recorder is made of: the app's (`app_parts`) or a test's (`test_support`).
pub(super) struct Parts {
    /// Makes the OS's capture backend; called once, on first use (it lists devices without capture).
    pub(super) backend: Box<dyn Fn() -> Arc<dyn Backend> + Send + Sync>,
    pub(super) launch: Arc<dyn Launch>,
    /// Runs the encoder probe, writing its report to the path given.
    pub(super) probe: Box<dyn Fn(&Path) + Send + Sync>,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) sink: Arc<dyn Sink>,
    pub(super) window: Arc<dyn Window>,
    /// The recorder's own files; None: no app data folder.
    pub(super) files: Option<Files>,
    /// Where takes are made; None: no videos folder.
    pub(super) root: Option<PathBuf>,
    /// The open project; None on the start screen.
    pub(super) open_project: Box<dyn Fn() -> Option<Opened> + Send + Sync>,
    /// A thread ticks the supervisor (the app); tests tick it themselves.
    pub(super) ticker: bool,
}

/// The project open when a take is armed: the take lands there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Opened {
    pub(super) id: String,
    /// Where it is saved, so the take can open it again should another be open by the time it lands.
    pub(super) file: PathBuf,
}

/// The OS's capture backend, made on first use.
struct LazyBackend {
    made: OnceLock<Arc<dyn Backend>>,
    make: Box<dyn Fn() -> Arc<dyn Backend> + Send + Sync>,
}

impl LazyBackend {
    fn get(&self) -> Arc<dyn Backend> {
        self.made.get_or_init(|| (self.make)()).clone()
    }
}

/// The recorder, outside a test run.
struct On {
    recorder: Arc<Recorder>,
    backend: Arc<LazyBackend>,
    probe: Box<dyn Fn(&Path) + Send + Sync>,
    files: Option<Files>,
    root: Option<PathBuf>,
    open_project: Box<dyn Fn() -> Option<Opened> + Send + Sync>,
    /// What the Record dialog was opened with: arming plans from it rather than listing the devices again.
    last_setup: Mutex<Option<Shown>>,
    /// One encoder probe at a time.
    probing: Mutex<()>,
    /// A probe gave no report since Klipolo opened (it hung or crashed): not waited for again until it opens again.
    probe_failed: AtomicBool,
}

pub struct RecorderState {
    /// None in a test run.
    on: Option<On>,
}

impl RecorderState {
    pub fn new(app: &AppHandle) -> Self {
        Self::build(test_run(), || app_parts(app))
    }

    /// The recorder from `parts`, or, in a `test_run`, none at all: `parts` isn't even made.
    pub(super) fn build(test_run: bool, parts: impl FnOnce() -> Parts) -> Self {
        if test_run {
            tracing::info!("recorder: off in this test run (KLIPOLO_TEST)");
            return Self { on: None };
        }
        let parts = parts();
        let backend = Arc::new(LazyBackend {
            made: OnceLock::new(),
            make: parts.backend,
        });
        let placed = backend.clone();
        let recorder = Recorder::new(Env {
            launch: parts.launch,
            clock: parts.clock,
            sink: parts.sink,
            window: parts.window,
            root: parts.root.clone(),
            place: Box::new(move || placed.get().caps().indicator),
            ticker: parts.ticker,
        });
        Self {
            on: Some(On {
                recorder,
                backend,
                probe: parts.probe,
                files: parts.files,
                root: parts.root,
                open_project: parts.open_project,
                last_setup: Mutex::new(None),
                probing: Mutex::new(()),
                probe_failed: AtomicBool::new(false),
            }),
        }
    }

    fn on(&self) -> Result<&On, CodedError> {
        self.on.as_ref().ok_or_else(words::test_run)
    }

    #[cfg(test)]
    pub(super) fn recorder(&self) -> Option<&Arc<Recorder>> {
        self.on.as_ref().map(|on| &on.recorder)
    }

    /// What the dialog was last shown.
    #[cfg(test)]
    pub(super) fn shown(&self) -> Option<Shown> {
        self.on.as_ref().and_then(|on| lock(&on.last_setup).clone())
    }

    /// The Record dialog's data: devices, permissions, each display's qualities and space, and the notes. No capture
    /// and no prompt; the encoder probe runs the first time, one at a time. While a take is the recorder's, neither the
    /// probe nor the folder's measure runs beside it. Arming then plans from this setup.
    pub(super) fn setup(&self) -> Result<RecorderSetup, CodedError> {
        let on = self.on()?;
        let files = on.files.as_ref().ok_or_else(words::no_settings_folder)?;
        let own_take = on.recorder.take_id().is_some();
        let backend = on.backend.get();
        let probing = if own_take {
            Probing::Kept
        } else {
            Probing::Run {
                run: &*on.probe,
                one_at_a_time: &on.probing,
                failed: &on.probe_failed,
            }
        };
        let root = on.root.as_deref();
        let shown = setup::recorder_setup(&*backend, files, probing, root, !own_take, own_take);
        let setup = shown.setup.clone();
        *lock(&on.last_setup) = Some(shown);
        Ok(setup)
    }

    /// Arms a take from the person's options: its worker waits for their Record, and nothing is captured. It lands in
    /// the open project, or in a new one when none is open. `capture.busy` while a take is the recorder's or Klipolo
    /// reopens; `capture.error.permission` until Screen Recording is allowed (Record would make macOS ask, and only the
    /// person's Allow press may).
    pub(super) fn arm(&self, options: &ArmOptions) -> Result<ArmedTake, CodedError> {
        let on = self.on()?;
        on.recorder.refuse_while_busy()?;
        let files = on.files.as_ref().ok_or_else(words::no_settings_folder)?;
        let backend = on.backend.get();
        if backend.permissions().screen != Permission::Granted {
            return Err(words::not_allowed());
        }
        let shown = lock(&on.last_setup).clone();
        // Not opened from the dialog: the setup now, with a kept probe report only and no folder measure.
        let shown = shown.unwrap_or_else(|| {
            setup::recorder_setup(&*backend, files, Probing::Kept, None, false, false)
        });
        let (target, project_file) = match (on.open_project)() {
            Some(Opened { id, file }) => (ImportTarget::Project { id }, Some(file)),
            None => (ImportTarget::New, None),
        };
        let created_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let id = klipolo_model::ids::new_id();
        let (cfg, notes) = setup::plan(&shown, options, &id, target, created_ms)?;
        on.recorder.arm(cfg, notes, project_file)
    }

    /// One command to take `take`'s worker.
    pub(super) fn send(&self, take: &str, cmd: Command) -> Result<(), CodedError> {
        self.on()?.recorder.send(take, cmd)
    }

    /// "Stop the running recording".
    pub(super) fn stop_running(&self) -> Result<Option<StoppedTake>, CodedError> {
        Ok(self.on()?.recorder.stop_running())
    }

    /// "Allow screen recording": the request, then the settings page with `open`.
    pub(super) fn allow_screen(&self, open: Open<'_>) -> Result<(), CodedError> {
        let on = self.on()?;
        permissions::allow_screen(&*on.backend.get(), open);
        Ok(())
    }

    /// "Open privacy settings": `page` with `open`.
    pub(super) fn open_privacy(&self, page: PrivacyPage, open: Open<'_>) -> Result<(), CodedError> {
        self.on()?;
        permissions::open_privacy(page, open)
    }

    /// "Reopen Klipolo": refused while an export runs (`exporting`, before and after the save) or during a take; else
    /// the project is saved (`flush`), then Klipolo `restart`s. No take arms meanwhile.
    pub(super) fn reopen(
        &self,
        exporting: &dyn Fn() -> bool,
        flush: impl FnOnce() -> Result<(), Said>,
        restart: impl FnOnce(),
    ) -> Result<(), Said> {
        let on = self.on()?;
        permissions::reopen(&on.recorder, on.files.as_ref(), exporting, flush, restart)
    }

    /// A take is the recorder's, armed through saving.
    pub(super) fn active(&self) -> bool {
        self.on.as_ref().is_some_and(|on| on.recorder.active())
    }

    /// Klipolo exits: a take that hasn't started is disarmed. True when one was.
    pub(super) fn disarm_unstarted(&self) -> bool {
        self.on
            .as_ref()
            .is_some_and(|on| on.recorder.disarm_unstarted())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The app's recorder: the OS's backend, this binary as the worker, the host clock, the page's events, Klipolo's main
/// window, `<app data>/recorder` and the recordings folder.
fn app_parts(app: &AppHandle) -> Parts {
    let files = (app.path().app_data_dir().ok()).map(|dir| Files {
        dir: dir.join("recorder"),
    });
    let exe = std::env::current_exe();
    let launch: Arc<dyn Launch> = match &exe {
        Ok(exe) => Arc::new(ProcessLaunch::new(exe.clone())),
        Err(error) => {
            tracing::error!(%error, "recorder: this app's binary can't be found");
            Arc::new(NoLaunch)
        }
    };
    let probe_exe = exe.ok();
    let projects = app.clone();
    let sink = AppSink::new(app.clone(), files.clone());
    Parts {
        backend: Box::new(|| Arc::from(klipolo_capture::platform::backend())),
        launch,
        probe: Box::new(move |out: &Path| match &probe_exe {
            Some(exe) => run_probe(exe, out),
            None => tracing::warn!("recorder: no binary to run the encoder probe"),
        }),
        clock: Arc::new(HostClock),
        sink: Arc::new(sink),
        window: Arc::new(MainWindow(app.clone())),
        files,
        root: klipolo_platform::paths::recordings_root(),
        open_project: Box::new(move || {
            let state = projects.try_state::<ProjectSessionState>()?;
            with_open(&state, |open| {
                Ok(Opened {
                    id: open.document.project().id.clone(),
                    file: open.file.clone(),
                })
            })
            .ok()
        }),
        ticker: true,
    }
}

/// How long the app waits for the encoder probe (the worker's own budget is 5 s).
const PROBE_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// `exe --record-worker --probe <out>`: the encoder probe on generated frames only (no capture, no prompt), stopped
/// after `PROBE_WAIT`.
fn run_probe(exe: &Path, out: &Path) {
    let started = std::time::Instant::now();
    let child = klipolo_platform::process::recorder_command(exe)
        .args(["--record-worker", "--probe"])
        .arg(out)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!(%error, "recorder: the encoder probe didn't start");
            return;
        }
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let took_ms = started.elapsed().as_millis();
                tracing::info!(%status, took_ms, "recorder: the encoder probe ran");
                return;
            }
            Ok(None) if started.elapsed() < PROBE_WAIT => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Ok(None) => {
                tracing::warn!("recorder: the encoder probe took too long, so it was stopped");
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            Err(error) => {
                tracing::warn!(%error, "recorder: the encoder probe's end couldn't be read");
                return;
            }
        }
    }
}

/// No binary to launch: every take is refused, with the reason in the log.
struct NoLaunch;

impl Launch for NoLaunch {
    fn spawn(&self, _args: &[String]) -> std::io::Result<WorkerIo> {
        Err(std::io::ErrorKind::NotFound.into())
    }
}
