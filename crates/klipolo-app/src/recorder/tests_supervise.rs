//! The supervisor: a hung worker is asked to stop, then killed, and resumed as a new run at most 3 times in 10
//! minutes; a worker slow to save is never taken for a hung one, and one that saved but doesn't exit is let go after its
//! grace; heartbeats reach the page only once the take is saving. Its own rules in virtual time, then through the recorder with fake workers: nothing captures.

use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::shell::WorkerState::*;
use klipolo_model::coded::CodedError;

use super::supervise::{EXIT_GRACE_MS, RESUME_WINDOW_MS, Supervisor, SupervisorAction};
use super::test_support::*;

const BEAT: Event = Event::Heartbeat {
    n: 1,
    queued_ms: 0,
    free_bytes: None,
};

#[test]
fn hung_worker_is_killed_and_resumed_at_most_3_times_in_10_minutes() {
    let rec = fixture();
    let id = rec.arm(options_screen()).unwrap().id;
    rec.send(Command::Start).unwrap();
    let folder = rec.take_dir(&id).to_string_lossy().into_owned();
    for resumes in 1..=3 {
        rec.launch.emit(state(Recording));
        // Asked to stop after 5 s without a sign of life, killed 2 s later, and resumed.
        rec.advance_s(8);
        assert_eq!(rec.launch.kills(), [false, true].repeat(resumes));
        assert_eq!(rec.launch.spawned(), 1 + resumes);
        assert_eq!(
            rec.launch.last_args(),
            [
                "--record-worker".to_string(),
                folder.clone(),
                "--resume".into()
            ]
        );
    }
    // The fourth hang in 10 minutes ends the take: it is finished from its files, and Klipolo comes back.
    rec.launch.emit(state(Recording));
    rec.advance_s(8);
    assert_eq!(rec.launch.spawned(), 4);
    let finished = rec.finished();
    assert!(finished.len() == 1 && finished[0].recover);
    assert!(rec.visible());
    assert_eq!(rec.rec().take_id(), None);
}

#[test]
fn the_resume_budget_is_per_ten_minutes() {
    let mut s = Supervisor::new(0);
    s.on_event(&state(Recording), 0);
    for at in [1_000, 2_000, 3_000] {
        assert_eq!(s.on_exit(Some(9), at), SupervisorAction::Resume);
        s.respawned(at);
    }
    assert_eq!(s.on_exit(Some(9), 4_000), SupervisorAction::GiveUp);
    let mut s = Supervisor::new(0);
    s.on_event(&state(Recording), 0);
    for at in [1_000, 2_000, 3_000] {
        assert_eq!(s.on_exit(Some(9), at), SupervisorAction::Resume);
        s.respawned(at);
    }
    // Resumes older than 10 minutes no longer count.
    assert_eq!(
        s.on_exit(Some(9), RESUME_WINDOW_MS + 1_500),
        SupervisorAction::Resume
    );
}

/// A worker slow to finish its Stop is never killed while it heartbeats, and once the person stopped, any exit is
/// finished from the files, never resumed.
#[test]
fn stop_has_no_time_limit_and_is_never_resumed() {
    let mut s = Supervisor::new(0);
    s.on_event(&state(Recording), 0);
    s.on_event(&state(Stopping), 1_000);
    for at in (2_000..60_000).step_by(1_000) {
        s.on_event(&BEAT, at);
        assert_eq!(
            s.on_tick(at + 900),
            None,
            "heartbeating through a 60 s save"
        );
    }
    assert_eq!(s.on_exit(Some(1), 61_000), SupervisorAction::Recover);
    let mut s = Supervisor::new(0);
    s.on_event(&state(Recording), 0);
    s.stop_requested();
    assert_eq!(s.on_exit(None, 1_000), SupervisorAction::Recover);
    // Finished from its files, it has no worker to watch.
    for at in (1_500..60_000).step_by(500) {
        assert_eq!(s.on_tick(at), None, "at {at} ms");
    }
}

#[test]
fn heartbeats_reach_the_page_once_the_take_is_saving() {
    let mut s = Supervisor::new(0);
    s.on_event(&state(Recording), 0);
    assert!(
        s.on_event(&BEAT, 1_000).is_empty(),
        "recording: not forwarded"
    );
    s.stop_requested();
    assert_eq!(s.on_event(&BEAT, 2_000), [SupervisorAction::Forward(BEAT)]);
}

/// Before the take records, an exit is the end of it: no resume, and the page hears why when the worker didn't say.
#[test]
fn a_worker_that_ends_before_recording_ends_the_take() {
    let mut s = Supervisor::new(0);
    s.on_event(&state(Armed), 0);
    assert_eq!(
        s.on_exit(Some(1), 1_000),
        SupervisorAction::Ended {
            failed: Some(CodedError::new("capture.error.os"))
        }
    );
    let mut s = Supervisor::new(0);
    assert_eq!(
        s.on_exit(Some(2), 1_000),
        SupervisorAction::Ended {
            failed: Some(CodedError::new("capture.error.unsupported"))
        }
    );
    let mut s = Supervisor::new(0);
    s.on_event(
        &Event::Failed {
            error: CodedError::new("capture.error.encoder"),
        },
        500,
    );
    assert_eq!(
        s.on_exit(Some(3), 1_000),
        SupervisorAction::Ended { failed: None },
        "it said why itself"
    );
}

/// A worker that saved the take (`Finished`) but never exits would hold the recorder for the whole session: after 15 s
/// it is asked to stop, 2 s later killed, and the recorder is free for the next take. Its files were complete, so the
/// take is landed as saved, never finished from its files.
#[test]
fn a_saved_take_whose_worker_never_exits_frees_the_recorder() {
    let rec = fixture();
    let id = rec.arm(options_screen()).unwrap().id;
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    rec.send(Command::Stop).unwrap();
    rec.launch.emit(state(Stopping));
    rec.launch.emit(Event::Finished {
        session: id.clone(),
    });
    // Heartbeats after it saved don't keep it alive.
    rec.launch.emit(BEAT);
    rec.advance_s(14);
    assert!(rec.launch.kills().is_empty(), "it may still be exiting");
    assert!(rec.state.active());
    rec.advance_s(1);
    assert_eq!(rec.launch.kills(), [false], "asked to stop");
    rec.advance_s(2);
    assert_eq!(rec.launch.kills(), [false, true], "then killed");
    rec.settle();
    assert_eq!(rec.rec().take_id(), None, "the recorder is free");
    assert!(!rec.state.active());
    let finished = rec.finished();
    assert!(finished.len() == 1 && !finished[0].recover, "{finished:?}");
    assert_eq!(rec.events().last(), Some(&state(Idle)));
    rec.arm(options_screen()).unwrap();
}

#[test]
fn a_saved_take_s_worker_is_asked_to_go_after_its_grace_only() {
    let mut s = Supervisor::new(0);
    s.on_event(&state(Recording), 0);
    s.on_event(&state(Stopping), 1_000);
    s.on_event(
        &Event::Finished {
            session: "t".into(),
        },
        2_000,
    );
    assert_eq!(s.on_tick(2_000 + EXIT_GRACE_MS - 1), None);
    assert_eq!(
        s.on_tick(2_000 + EXIT_GRACE_MS),
        Some(SupervisorAction::Kill { hard: false })
    );
    s.on_event(&BEAT, 2_000 + EXIT_GRACE_MS + 500);
    assert_eq!(
        s.on_tick(2_000 + EXIT_GRACE_MS + 2_000),
        Some(SupervisorAction::Kill { hard: true }),
        "a heartbeat doesn't undo the request"
    );
}
