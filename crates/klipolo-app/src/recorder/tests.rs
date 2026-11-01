//! The recorder's functional tests: one worker per take and only the person's Start, stale and refused commands, the
//! page's events, the window, where a take lands, and the test run that turns the recorder off. The fake backend and
//! fake workers only: nothing captures and nothing is asked of the OS.

use std::sync::atomic::{AtomicBool, Ordering};

use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::session::ImportTarget;
use klipolo_capture::shell::WorkerState::*;
use klipolo_capture::synthetic::Script;

use super::state::{RecorderState, test_run};
use super::test_support::*;
use super::types::PrivacyPage;

#[test]
fn arm_spawns_one_worker_for_the_take_folder_and_nothing_starts() {
    let rec = fixture();
    let armed = rec.arm(options_screen()).unwrap();
    assert_eq!(armed.countdown_s, 3);
    assert_eq!(rec.launch.spawned(), 1);
    let folder = rec.take_dir(&armed.id);
    assert!(folder.join("session.json").is_file(), "its config first");
    assert_eq!(
        rec.launch.last_args(),
        [
            "--record-worker".to_string(),
            folder.to_string_lossy().into()
        ]
    );
    rec.settle();
    assert!(rec.launch.commands().is_empty(), "no line on its stdin");
    // Only the person's Record press starts it.
    rec.send(Command::Start).unwrap();
    assert_eq!(rec.launch.commands(), [Command::Start]);
}

/// A worker that can't be launched says so, not that the Movies folder can't be written to, and leaves no folder.
#[test]
fn a_worker_that_cannot_launch_says_the_recorder_didnt_start() {
    let rec = fixture();
    rec.launch.0.refuse.store(true, Ordering::SeqCst);
    let error = rec.arm(options_screen()).unwrap_err();
    assert_eq!(error.code, "capture.error.workerDidntStart");
    assert_eq!(
        klipolo_capture::english::english(&error),
        "The recorder didn't start. Reopen Klipolo, then press Record again."
    );
    let left = std::fs::read_dir(&rec.root).map_or(0, |d| d.count());
    assert_eq!(left, 0, "no take folder left");
    assert!(!rec.state.active());
    rec.launch.0.refuse.store(false, Ordering::SeqCst);
    rec.arm(options_screen()).unwrap();
}

#[test]
fn second_arm_while_a_take_runs_is_busy() {
    let rec = fixture();
    rec.arm(options_screen()).unwrap();
    let busy = rec.arm(options_screen()).unwrap_err();
    assert_eq!(busy.code, "capture.busy");
    assert_eq!(rec.launch.spawned(), 1, "no second worker");
    // Busy through saving: a take that is stopping is still the recorder's.
    rec.launch.emit(state(Recording));
    rec.send(Command::Stop).unwrap();
    rec.launch.emit(state(Stopping));
    assert_eq!(rec.arm(options_screen()).unwrap_err().code, "capture.busy");
    // Once it has ended, the next take arms.
    rec.launch.emit(Event::Finished {
        session: "t".into(),
    });
    rec.launch.exit(Some(0));
    rec.settle();
    rec.arm(options_screen()).unwrap();
    assert_eq!(rec.launch.spawned(), 2);
}

#[test]
fn commands_for_another_take_are_stale() {
    let rec = fixture();
    let first = rec.arm(options_screen()).unwrap().id;
    rec.send(Command::Disarm).unwrap();
    rec.launch.exit(Some(3));
    rec.settle();
    let second = rec.arm(options_screen()).unwrap().id;
    let stale = rec.state.send(&first, Command::Cancel).unwrap_err();
    assert_eq!(stale.code, "capture.error.staleTake");
    assert_eq!(
        rec.launch.commands(),
        [Command::Disarm],
        "never reaches the new take"
    );
    // The page may send only the take's own commands.
    for refused in [
        Command::Marker,
        Command::StartAgain,
        Command::ViewSize { view: None },
    ] {
        let error = rec.state.send(&second, refused.clone()).unwrap_err();
        assert_eq!(error.code, "capture.error.unsupported", "{refused:?}");
    }
    for allowed in [
        Command::WithoutSource {
            source: "mic".into(),
        },
        Command::Start,
        Command::Pause,
        Command::Resume,
        Command::Cancel,
    ] {
        rec.state.send(&second, allowed.clone()).unwrap();
    }
    assert_eq!(rec.launch.commands().len(), 6);
}

#[test]
fn worker_events_reach_the_page_with_their_take() {
    let rec = fixture();
    let id = rec.arm(options_screen()).unwrap().id;
    rec.send(Command::Start).unwrap();
    let said = [
        state(Starting),
        Event::Countdown { left_s: 3 },
        Event::Meter {
            source: "mic".into(),
            peak_dbfs: -20.0,
        },
        state(Recording),
    ];
    for ev in said.clone() {
        rec.launch.emit(ev);
    }
    // While it records, its heartbeats are the supervisor's own.
    rec.launch.emit(Event::Heartbeat {
        n: 1,
        queued_ms: 0,
        free_bytes: None,
    });
    let tagged = rec.tagged_events();
    let expected: Vec<(String, Event)> = said.into_iter().map(|ev| (id.clone(), ev)).collect();
    assert_eq!(tagged, expected);
}

#[test]
fn window_hides_while_recording_and_comes_back_after_stop_failure_or_cancel() {
    // Stop: hidden from the first Recording (a pause and resume keep it hidden), back once the take is saved.
    let rec = fixture();
    rec.arm(options_screen()).unwrap();
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Countdown));
    assert!(rec.visible(), "the countdown shows in Klipolo");
    rec.launch.emit(state(Recording));
    assert!(!rec.visible(), "hidden while recording");
    rec.launch.emit(state(Paused));
    rec.launch.emit(state(Recording));
    assert!(!rec.visible());
    rec.send(Command::Stop).unwrap();
    rec.launch.emit(state(Stopping));
    assert!(!rec.visible(), "still hidden while it saves");
    rec.launch.emit(Event::Finished {
        session: "t".into(),
    });
    assert!(rec.visible(), "back once it is saved");
    assert_eq!(rec.window.hides.load(Ordering::SeqCst), 1);

    // A failure: back at once.
    let rec = fixture();
    rec.arm(options_screen()).unwrap();
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    assert!(!rec.visible());
    rec.launch.emit(Event::Failed {
        error: klipolo_model::coded::CodedError::new("capture.error.driveStuck"),
    });
    assert!(rec.visible());

    // Cancel: the take ends with no recording, and its `State Idle` brings the window back.
    let rec = fixture();
    let id = rec.arm(options_screen()).unwrap().id;
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    assert!(!rec.visible());
    rec.send(Command::Cancel).unwrap();
    rec.launch.exit(Some(3));
    assert!(rec.visible());
    assert_eq!(rec.tagged_events().last(), Some(&(id, state(Idle))));
}

#[test]
fn no_open_project_arms_for_a_new_one() {
    let rec = fixture();
    let id = rec.arm(options_screen()).unwrap().id;
    assert_eq!(
        rec.config(&id).target,
        ImportTarget::Project {
            id: "project-1".into()
        },
        "the open project"
    );
    rec.send(Command::Disarm).unwrap();
    rec.launch.exit(Some(3));
    rec.settle();
    // From the start screen: a new project, made only when the take lands.
    *rec.project.lock().unwrap() = None;
    let id = rec.arm(options_screen()).unwrap().id;
    assert_eq!(rec.config(&id).target, ImportTarget::New);
}

#[test]
fn the_recorder_is_active_from_arming_through_saving() {
    let rec = fixture();
    assert!(!rec.state.active());
    rec.arm(options_screen()).unwrap();
    assert!(rec.state.active(), "armed: the mic may open at any moment");
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    rec.send(Command::Stop).unwrap();
    rec.launch.emit(state(Stopping));
    rec.settle();
    assert!(rec.state.active(), "saving");
    rec.launch.emit(Event::Finished {
        session: "t".into(),
    });
    rec.launch.exit(Some(0));
    rec.settle();
    assert!(!rec.state.active());
}

/// Klipolo exits: a take that hasn't started is disarmed, so no armed worker outlives the app with ⌥⌘R registered. A
/// take past its start records on, orphaned, and stops from its own menu-bar item.
#[test]
fn exiting_disarms_a_take_that_has_not_started() {
    for said in [None, Some(Armed), Some(Starting), Some(Countdown)] {
        let rec = fixture();
        rec.arm(options_screen()).unwrap();
        if said.is_some() && said != Some(Armed) {
            rec.send(Command::Start).unwrap();
        }
        if let Some(s) = said {
            rec.launch.emit(state(s));
        }
        rec.settle();
        assert!(rec.state.disarm_unstarted(), "{said:?}");
        assert_eq!(
            rec.launch.commands().last(),
            Some(&Command::Disarm),
            "{said:?}"
        );
        // Sent once.
        assert!(!rec.state.disarm_unstarted());
    }
    let rec = fixture();
    rec.arm(options_screen()).unwrap();
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    rec.settle();
    assert!(
        !rec.state.disarm_unstarted(),
        "recording: left to its worker"
    );
    assert_eq!(rec.launch.commands(), [Command::Start]);
    let rec = fixture();
    assert!(!rec.state.disarm_unstarted(), "no take");
}

#[test]
fn test_run_never_makes_a_backend_or_worker() {
    assert!(
        test_run(),
        "every test command sets KLIPOLO_TEST, and the recorder reads it"
    );
    let dir = tempfile::tempdir().unwrap();
    let (parts, world) = world(dir.path(), Script::default());
    let built = AtomicBool::new(false);
    let state = RecorderState::build(test_run(), || {
        built.store(true, Ordering::SeqCst);
        parts
    });
    let test_run_code = "capture.error.testRun";
    assert_eq!(state.setup().unwrap_err().code, test_run_code);
    assert_eq!(
        state.arm(&options_screen()).unwrap_err().code,
        test_run_code
    );
    assert_eq!(
        state.send("t", Command::Start).unwrap_err().code,
        test_run_code
    );
    assert_eq!(state.stop_running().unwrap_err().code, test_run_code);
    let opened = std::sync::Mutex::new(Vec::<String>::new());
    let open = |url: &str| {
        opened.lock().unwrap().push(url.into());
        Ok(())
    };
    assert_eq!(state.allow_screen(&open).unwrap_err().code, test_run_code);
    assert_eq!(
        state
            .open_privacy(PrivacyPage::Camera, &open)
            .unwrap_err()
            .code,
        test_run_code
    );
    let flushed = AtomicBool::new(false);
    let refused = state.reopen(
        &|| false,
        || {
            flushed.store(true, Ordering::SeqCst);
            Ok(())
        },
        || panic!("never restarts"),
    );
    assert_eq!(code_of(&refused.unwrap_err()), test_run_code);
    assert!(!state.active());
    assert!(!state.disarm_unstarted());
    assert!(
        !built.load(Ordering::SeqCst),
        "no backend, no launch, no probe"
    );
    assert_eq!(world.backends.load(Ordering::SeqCst), 0);
    assert_eq!(world.launch.spawned(), 0);
    assert_eq!(world.probes.load(Ordering::SeqCst), 0);
    assert!(opened.lock().unwrap().is_empty() && !flushed.load(Ordering::SeqCst));
    assert_eq!(
        klipolo_capture::english::english(&klipolo_model::coded::CodedError::new(test_run_code)),
        "Recording is off in this test run."
    );
}
