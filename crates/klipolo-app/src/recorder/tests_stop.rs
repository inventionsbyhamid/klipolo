//! How takes end. Stop is taken in every state the worker reports, "Stop the running recording" reaches whichever take
//! runs, and a take whose encoder gives up mid-take, or whose worker keeps crashing, ends with plain words and keeps
//! what was written. Fake workers only: nothing captures.

use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::shell::Level;
use klipolo_capture::shell::WorkerState::{self, *};
use klipolo_model::coded::{CodedError, ErrorParam};

use super::test_support::*;
use super::types::StoppedTake;

/// What a take's worker has said by the time it reaches `state`, in order.
fn said_until(state: WorkerState) -> Vec<WorkerState> {
    let order = [Armed, Starting, Countdown, Recording];
    let mut said: Vec<_> = order.iter().copied().take_while(|s| *s != state).collect();
    match state {
        Paused => said.push(Paused),
        Stopping => said.push(Stopping),
        Ready | Failed => said.extend([Stopping, state]),
        _ => said.push(state),
    }
    said
}

/// In every state, Stop for the take is taken (never refused) and reaches its worker. A worker that then ends without
/// finishing is never resumed: before it recorded, the take ends with the page's one `State Idle`; after, the take is
/// finished from its files.
#[test]
fn stop_is_taken_in_every_state_the_worker_reports() {
    for until in [
        Armed, Starting, Countdown, Recording, Paused, Stopping, Ready, Failed,
    ] {
        let rec = fixture();
        let id = rec.arm(options_screen()).unwrap().id;
        let said = said_until(until);
        for s in &said {
            if *s == Starting {
                rec.send(Command::Start).unwrap();
            }
            rec.launch.emit(state(*s));
        }
        rec.send(Command::Stop)
            .unwrap_or_else(|e| panic!("Stop refused while {until:?}: {}", e.code));
        assert_eq!(
            rec.launch.commands().last(),
            Some(&Command::Stop),
            "{until:?}"
        );
        rec.launch.exit(Some(1));
        rec.settle();
        assert_eq!(
            rec.launch.spawned(),
            1,
            "{until:?}: never resumed after Stop"
        );
        let finished = rec.finished();
        if said.contains(&Recording) {
            assert!(
                finished.len() == 1 && finished[0].recover,
                "{until:?}: finished from its files"
            );
            assert!(rec.take_dir(&id).is_dir(), "{until:?}: its folder stays");
        } else {
            assert!(finished.is_empty(), "{until:?}: nothing to land");
            assert_eq!(rec.tagged_events().last(), Some(&(id, state(Idle))));
        }
        assert!(rec.visible(), "{until:?}: Klipolo is back");
        assert_eq!(
            rec.rec().take_id(),
            None,
            "{until:?}: the next take can be armed"
        );
    }
}

/// "Stop the running recording" stops whatever take the recorder runs, whichever id the page lost, and says whether it
/// had started (only then is it saved).
#[test]
fn stop_running_stops_the_current_take_whatever_its_id() {
    let rec = fixture();
    assert_eq!(rec.state.stop_running().unwrap(), None, "nothing to stop");
    let id = rec.arm(options_screen()).unwrap().id;
    rec.launch.emit(state(Armed));
    rec.settle();
    let armed = StoppedTake {
        id: id.clone(),
        started: false,
    };
    assert_eq!(rec.state.stop_running().unwrap(), Some(armed));
    assert_eq!(rec.launch.commands(), [Command::Stop]);

    let rec = fixture();
    let id = rec.arm(options_screen()).unwrap().id;
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    rec.settle();
    assert_eq!(
        rec.state.stop_running().unwrap(),
        Some(StoppedTake { id, started: true })
    );
}

/// `capture.health.lost` for `source` after its encoder kept failing: the worker rebuilt it, stepped it down, then gave
/// it up.
fn encoder_gave_up(source: &str) -> Event {
    let mut note = CodedError::new("capture.health.lost");
    note.params.insert(
        "source".into(),
        ErrorParam::from(CodedError::new(&format!("capture.source.{source}"))),
    );
    note.params
        .insert("reason".into(), ErrorParam::from("encoder"));
    note.params
        .insert("status".into(), ErrorParam::from(-536_870_212_i64));
    Event::Health {
        note,
        level: Level::Warning,
        source: Some(source.into()),
    }
}

/// The macOS encoder can fail mid-take (two H.264 sessions on one engine, the screen's and the camera's). Once the
/// worker gives a source up for its encoder, the take stops and is saved: the page hears why in plain words, and
/// Klipolo comes back with the take, whose folder holds everything written so far.
#[test]
fn an_encoder_that_gives_up_mid_take_stops_and_saves_the_take() {
    for source in ["camera", "screen"] {
        let rec = fixture();
        let mut options = options_screen();
        options.camera = Some("camera-1".into());
        let id = rec.arm(options).unwrap().id;
        rec.send(Command::Start).unwrap();
        rec.launch.emit(state(Recording));
        assert!(!rec.visible());
        rec.launch.emit(encoder_gave_up(source));
        rec.settle();
        assert_eq!(
            rec.launch.commands(),
            [Command::Start, Command::Stop],
            "{source}: stopped by the app"
        );
        let stopped = Event::Health {
            note: CodedError::new("capture.stop.encoder"),
            level: Level::Critical,
            source: Some(source.into()),
        };
        let events = rec.events();
        assert_eq!(
            &events[events.len() - 2..],
            [encoder_gave_up(source), stopped]
        );
        assert_eq!(
            klipolo_capture::english::english(&CodedError::new("capture.stop.encoder")),
            "The video encoder stopped, so the recording stopped and was saved."
        );
        // A second note from the worker never stops it twice.
        rec.launch.emit(encoder_gave_up(source));
        rec.settle();
        assert_eq!(rec.launch.commands().len(), 2);
        // The worker saves the take, and Klipolo comes back with it.
        rec.launch.emit(state(Stopping));
        rec.launch.emit(Event::Finished {
            session: id.clone(),
        });
        rec.launch.exit(Some(0));
        let finished = rec.finished();
        assert!(finished.len() == 1 && !finished[0].recover, "{source}");
        assert!(rec.take_dir(&id).is_dir(), "{source}: its folder stays");
        assert!(rec.visible(), "{source}");
    }
    // A worker that dies while it saves is never resumed: the take is finished from its files.
    let rec = fixture();
    let id = rec.arm(options_screen()).unwrap().id;
    rec.send(Command::Start).unwrap();
    rec.launch.emit(state(Recording));
    rec.launch.emit(encoder_gave_up("screen"));
    rec.launch.exit(None);
    rec.settle();
    assert_eq!(rec.launch.spawned(), 1, "never resumed");
    let finished = rec.finished();
    assert!(finished.len() == 1 && finished[0].recover);
    assert!(rec.take_dir(&id).is_dir());
    assert!(rec.visible());
}

/// A worker that keeps crashing (the encoder can take the process with it) is resumed 3 times in 10 minutes; the
/// fourth crash ends the take with plain words, and it is finished from its files rather than left behind.
#[test]
fn a_worker_that_keeps_crashing_ends_the_take_and_keeps_it() {
    let rec = fixture();
    let id = rec.arm(options_screen()).unwrap().id;
    rec.send(Command::Start).unwrap();
    for _ in 0..4 {
        rec.launch.emit(state(Recording));
        rec.launch.exit(Some(9));
        rec.advance_s(10);
    }
    assert_eq!(rec.launch.spawned(), 4, "the take plus three resumes");
    let finished = rec.finished();
    assert!(finished.len() == 1 && finished[0].recover, "{finished:?}");
    assert!(rec.take_dir(&id).is_dir());
    let events = rec.events();
    let crash_loop = Event::Health {
        note: CodedError::new("capture.stop.crashLoop"),
        level: Level::Critical,
        source: None,
    };
    assert!(events.contains(&crash_loop), "{events:?}");
    assert_eq!(events.last(), Some(&state(Idle)));
    assert!(rec.visible());
    assert_eq!(rec.rec().take_id(), None);
}
