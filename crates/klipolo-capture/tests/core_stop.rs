//! Stop is always respected: the recorder UI's "Stop recording" sends Stop to a take that may be in any
//! state, so `SessionCore` takes it from every one. No process is spawned here.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::{Harness, Src, cfg, mics};
use klipolo_capture::protocol::Command;
use klipolo_capture::run::core::{Effect, SessionCore};
use klipolo_capture::shell::WorkerState;

const SOURCES: &[Src] = &[Src::Screen, Src::Mic];

fn finalized(h: &Harness) -> usize {
    (h.effects().iter())
        .filter(|e| matches!(e, Effect::Finalize))
        .count()
}

#[test]
fn stop_is_taken_from_every_state() {
    // Before the start gate there is no take: nothing written, every pipe stops, and the worker exits 3.
    let armed = Harness::new(cfg(SOURCES, 3));
    let mut countdown = Harness::new(cfg(SOURCES, 3));
    countdown.command(Command::Start);
    countdown.feed(0.0, 1.5);
    for (mut h, state) in [
        (armed, WorkerState::Armed),
        (countdown, WorkerState::Countdown),
    ] {
        assert_eq!(h.state(), state);
        h.command(Command::Stop);
        assert!(
            h.effects().iter().any(|e| matches!(e, Effect::Exit(3))),
            "{state:?}: ends with no take"
        );
        assert!(h.effects().iter().any(|e| matches!(e, Effect::StopPipes)));
        assert!(h.file_keys().is_empty(), "{state:?}: nothing written");
        assert_eq!(finalized(&h), 0, "{state:?}");
    }
    // Starting: its pipes haven't answered yet (the harness's answer at once, so the core is driven bare here).
    let mut core = SessionCore::new(cfg(SOURCES, 3), mics(), 0);
    core.opening(0);
    core.command(Command::Start, 1);
    assert_eq!(core.state(), WorkerState::Starting);
    let fx = core.command(Command::Stop, 2);
    assert!(fx.iter().any(|e| matches!(e, Effect::StopPipes)));
    assert!(
        fx.iter().any(|e| matches!(e, Effect::Exit(3))),
        "Starting: ends with no take"
    );
    assert!(!fx.iter().any(|e| matches!(e, Effect::Finalize)));

    // Recording or paused: the take is saved.
    let recording = Harness::recording(SOURCES);
    let mut paused = Harness::recording(SOURCES);
    paused.command(Command::Pause);
    for (mut h, state) in [
        (recording, WorkerState::Recording),
        (paused, WorkerState::Paused),
    ] {
        assert_eq!(h.state(), state);
        h.command(Command::Stop);
        assert_eq!(h.state(), WorkerState::Stopping, "{state:?}");
        assert_eq!(finalized(&h), 1, "{state:?}: saved once");
    }

    // Saving already: another Stop changes nothing, and the take is saved once.
    let mut saving = Harness::recording(SOURCES);
    saving.command(Command::Stop);
    saving.command(Command::Stop);
    assert_eq!(saving.state(), WorkerState::Stopping);
    assert_eq!(finalized(&saving), 1);
}
