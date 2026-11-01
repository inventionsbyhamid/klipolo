//! A system quit while the worker runs: the shell asks the session to Stop and waits for
//! its end without returning first, as AppKit does inside `terminate:`, and still learns the session's own exit code
//! once the take is saved. Pure: how long it waits in each state.
#![cfg(feature = "synthetic")]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::harness::{Src, fake_backend, session};
use klipolo_capture::backend::{
    AudioSpec, Backend, CaptureCaps, Devices, Permissions, PreviewPipe, ProbeReport, VideoPipes,
    VideoSpec,
};
use klipolo_capture::manifest::Manifest;
use klipolo_capture::pipe::{AudioPipe, PipeError};
use klipolo_capture::protocol::Command;
use klipolo_capture::run::threads::run_with;
use klipolo_capture::shell::{MenuAction, Shell, ShellInput, ShellIo, ShellOutput, WorkerState};
use klipolo_capture::shell_view::{QUIT_IDLE_WAIT, QUIT_TAKE_WAIT, quit_wait};

#[test]
fn a_take_and_a_start_get_ten_seconds_to_stop_and_save_and_no_take_ends_at_once() {
    for state in [
        WorkerState::Starting,
        WorkerState::Countdown,
        WorkerState::Recording,
        WorkerState::Paused,
        WorkerState::Stopping,
    ] {
        assert_eq!(quit_wait(Some(state)), QUIT_TAKE_WAIT, "{state:?}");
    }
    for state in [
        None,
        Some(WorkerState::Armed),
        Some(WorkerState::Ready),
        Some(WorkerState::Failed),
    ] {
        assert_eq!(quit_wait(state), QUIT_IDLE_WAIT, "{state:?}");
    }
}

#[test]
fn a_system_quit_mid_take_saves_it_and_ends_with_the_session_s_own_code() {
    let root = tempfile::tempdir().unwrap();
    let dir = session(root.path(), &[Src::Screen, Src::Mic]);
    let got = Arc::new(Mutex::new(None));
    let backend = Box::new(Quitting {
        inner: fake_backend(),
        got: got.clone(),
    });
    let code = run_with(backend, dir.clone(), false, vec![(0, Command::Start)]);
    assert_eq!(code, 0, "the take finished");
    assert_eq!(
        *got.lock().unwrap(),
        Some(Some(0)),
        "the shell learned the code before returning"
    );
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(dir.manifest()).unwrap()).unwrap();
    assert!(
        manifest
            .sources
            .iter()
            .any(|s| s.id == "screen" && !s.runs.is_empty()),
        "the screen was saved"
    );
}

/// The system's quit arrives on the second second of recording: the menu's Stop, then the session's end awaited
/// inside the shell, within `quit_wait`.
struct QuitShell {
    got: Arc<Mutex<Option<Option<i32>>>>,
}

impl Shell for QuitShell {
    fn run(self: Box<Self>, io: ShellIo) {
        let (mut recording, mut wait) = (0, QUIT_IDLE_WAIT);
        for out in io.output.iter() {
            if let ShellOutput::Status(v) = &out
                && v.state == WorkerState::Recording
            {
                recording += 1;
                if recording == 2 {
                    wait = quit_wait(Some(v.state));
                    io.input.send(ShellInput::Menu(MenuAction::Stop)).unwrap();
                }
            }
            if out == ShellOutput::Quit {
                break;
            }
        }
        *self.got.lock().unwrap() = Some(io.end.wait(wait));
    }
}

/// The fake backend with the quitting shell.
struct Quitting {
    inner: Box<dyn Backend>,
    got: Arc<Mutex<Option<Option<i32>>>>,
}

impl Backend for Quitting {
    fn caps(&self) -> CaptureCaps {
        self.inner.caps()
    }
    fn devices(&self) -> Devices {
        self.inner.devices()
    }
    fn permissions(&self) -> Permissions {
        self.inner.permissions()
    }
    fn video(&self, spec: &VideoSpec) -> Result<VideoPipes, PipeError> {
        self.inner.video(spec)
    }
    fn audio(&self, spec: &AudioSpec) -> Result<Box<dyn AudioPipe>, PipeError> {
        self.inner.audio(spec)
    }
    fn preview(&self, camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        self.inner.preview(camera)
    }
    fn probe(&self, budget: Duration) -> ProbeReport {
        self.inner.probe(budget)
    }
    fn shell(&self) -> Box<dyn Shell> {
        Box::new(QuitShell {
            got: self.got.clone(),
        })
    }
}
