//! A source that didn't start at Record, or whose permission was pulled before the gate.
//!
//! - **The main picture** (the screen, window or game; the camera in Camera only): no take. The other pipes stop, the
//!   empty folder goes (exit 3) and the app hears `Failed`, `capture.permission.screen` when Screen Recording wasn't
//!   allowed, else the pipe's own error.
//! - **Any other source** leaves the start gate at once with its note, and the take goes on
//!   the gate never asks "{source} isn't sending anything yet" about it 3 s later. The camera beside a
//!   screen is no exception (every backend that lists a camera records it, so none is refused as "not yet").
#![cfg(feature = "synthetic")]

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};

use common::didnt_start;
use common::harness::{Harness, Src, cfg};
use klipolo_capture::pipe::{PipeError, PipeEvent};
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::run::check::check_camera_with;
use klipolo_capture::run::core::Effect;
use klipolo_capture::shell::WorkerState;
use klipolo_capture::synthetic::{FakeBackend, Script};
use klipolo_model::project::RecordingKind;

/// No take, with `error`: the app heard one `Failed`, nothing recorded or written, every pipe told to stop, and the
/// worker exits 3 (its threads layer removes the empty folder).
fn no_take(h: &Harness, error: &str) {
    let failed: Vec<&str> = (h.events().iter())
        .filter_map(|e| match e {
            Event::Failed { error } => Some(error.code.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(failed, [error], "{:?}", h.events());
    let recorded = (h.events().iter()).any(|e| {
        matches!(
            e,
            Event::State {
                state: WorkerState::Recording,
                ..
            }
        )
    });
    assert!(!recorded, "it never recorded: {:?}", h.events());
    assert_eq!(h.state(), WorkerState::Stopping);
    assert!(h.file_keys().is_empty(), "written: {:?}", h.file_keys());
    for source in h.cfg.sources.iter().map(|s| s.id.as_str()) {
        assert!(h.stopped(source), "{source} was never told to stop");
    }
    assert!(h.effects().contains(&Effect::Exit(3)), "{:?}", h.effects());
}

#[test]
fn a_screen_without_permission_records_nothing_though_the_camera_started() {
    // A Mac where Screen Recording was never allowed: the camera's pipe starts first and is live when
    // the screen and the computer's sound (on the screen's stream) fail.
    let srcs = [Src::Camera, Src::Screen, Src::ComputerSound, Src::Mic];
    let mut h = Harness::new(cfg(&srcs, 3));
    h.fails_to_start("screen", PipeError::PermissionDenied);
    h.fails_to_start("computer-sound", PipeError::PermissionDenied);
    h.command(Command::Start);
    h.feed(0.0, 5.0);
    no_take(&h, "capture.permission.screen");
}

#[test]
fn a_screen_that_cant_start_for_another_reason_ends_with_its_own_error() {
    let mut h = Harness::new(cfg(&[Src::Camera, Src::Screen, Src::Mic], 3));
    h.fails_to_start("screen", PipeError::Encoder(-12_900));
    h.command(Command::Start);
    h.feed(0.0, 5.0);
    no_take(&h, "capture.error.encoder");
}

#[test]
fn camera_only_whose_camera_cant_start_records_nothing() {
    // The camera's own permission is named for it, never the screen's.
    for (error, said) in [
        (PipeError::Busy, "capture.error.busy"),
        (PipeError::PermissionDenied, "capture.permission.camera"),
    ] {
        let mut config = cfg(&[Src::Mic, Src::Camera], 3);
        config.kind = RecordingKind::Camera;
        let mut h = Harness::new(config);
        h.fails_to_start("camera", error);
        h.command(Command::Start);
        h.feed(0.0, 5.0);
        no_take(&h, said);
    }
}

#[test]
fn computer_sound_that_didnt_start_is_noted_and_the_take_goes_on() {
    let mut h = Harness::new(cfg(&[Src::Screen, Src::ComputerSound, Src::Mic], 3));
    h.fails_to_start("computer-sound", PipeError::PermissionDenied);
    h.command(Command::Start);
    h.feed(0.0, 5.0);
    let noted = (h.events().iter())
        .filter(|e| matches!(e, Event::Health { note, .. } if didnt_start(note, "computerSound", "capture.error.permission")))
        .count();
    assert_eq!(noted, 1, "noted, by name, as it failed: {:?}", h.events());
    assert!(
        !(h.events().iter()).any(|e| matches!(e, Event::Failed { .. } | Event::Gate { .. })),
        "never refused, never asked about: {:?}",
        h.events()
    );
    assert_eq!(h.state(), WorkerState::Recording);
    let runs = |s| h.run_count(s);
    assert_eq!(
        (runs("screen"), runs("computer-sound"), runs("mic")),
        (1, 0, 1)
    );
}

#[test]
fn screen_recording_pulled_before_the_gate_is_a_refusal() {
    // The person turns Klipolo off in System Settings during the countdown: no take, never "was
    // saved", and the step's code.
    let mut h = Harness::new(cfg(&[Src::Camera, Src::Screen, Src::Mic], 3));
    h.command(Command::Start);
    assert_eq!(h.state(), WorkerState::Countdown);
    h.pipe_event("screen", PipeEvent::PermissionRevoked);
    h.feed(0.0, 5.0);
    no_take(&h, "capture.permission.screen");
    // The camera's pulled meanwhile: it is noted and left out, and the take goes on.
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Camera, Src::Mic], 3));
    h.command(Command::Start);
    h.pipe_event("camera", PipeEvent::PermissionRevoked);
    h.feed(0.0, 5.0);
    let noted = (h.events().iter())
        .filter(|e| matches!(e, Event::Health { note, .. } if didnt_start(note, "camera", "capture.error.permission")))
        .count();
    assert_eq!(noted, 1, "{:?}", h.events());
    assert!(!(h.events().iter()).any(|e| matches!(e, Event::Failed { .. })));
    assert_eq!(h.state(), WorkerState::Recording);
    assert_eq!((h.run_count("screen"), h.run_count("camera")), (1, 0));
    assert!(h.stopped("camera"), "its pipe was told to stop");
}

#[test]
fn stop_sharing_before_the_gate_is_a_refusal_never_said_to_be_saved() {
    // The system's "Stop sharing" during the countdown (carried): no take, and the words say so.
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Mic], 3));
    h.command(Command::Start);
    assert_eq!(h.state(), WorkerState::Countdown);
    h.pipe_event("screen", PipeEvent::StoppedByUser);
    h.feed(0.0, 5.0);
    no_take(&h, "capture.stop.sharingBeforeStart");
    let said = klipolo_capture::english::english(&klipolo_model::coded::CodedError::new(
        "capture.stop.sharingBeforeStart",
    ));
    assert!(
        said.contains("nothing was recorded") && !said.contains("saved"),
        "{said}"
    );
}

#[test]
fn a_camera_that_cant_start_is_noted_and_the_take_records_the_rest() {
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Camera, Src::Mic], 3));
    h.fails_to_start("camera", PipeError::Unsupported);
    h.command(Command::Start);
    h.feed(0.0, 5.0);
    let noted = (h.events().iter())
        .filter(|e| matches!(e, Event::Health { note, .. } if didnt_start(note, "camera", "capture.error.unsupported")))
        .count();
    assert_eq!(noted, 1, "{:?}", h.events());
    assert!(
        !(h.events().iter()).any(|e| matches!(e, Event::Failed { .. } | Event::Gate { .. })),
        "never refused, never asked about: {:?}",
        h.events()
    );
    assert_eq!(h.state(), WorkerState::Recording);
    assert_eq!((h.run_count("screen"), h.run_count("mic")), (1, 1));
}

#[test]
fn check_camera_that_cant_open_says_why() {
    let backend = FakeBackend::new(Script {
        start_errors: BTreeMap::from([("camera".into(), PipeError::Unsupported)]),
        ..Script::default()
    });
    // The app's end stays open while the camera starts.
    let (reader, _app) = std::io::pipe().unwrap();
    let input: Box<dyn BufRead + Send> = Box::new(BufReader::new(reader));
    let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let code = check_camera_with(
        Box::new(backend),
        "camera-1",
        input,
        Box::new(Shared(out.clone())),
    );
    let failed: Vec<String> = (out.lock().unwrap().lines().map_while(Result::ok))
        .filter_map(|line| match serde_json::from_str(&line) {
            Ok(Event::Failed { error }) => Some(error.code),
            _ => None,
        })
        .collect();
    assert_eq!(code, 1);
    assert_eq!(failed, ["capture.error.unsupported"]);
}

/// Check camera's output, shared with the test.
struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Shared {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
