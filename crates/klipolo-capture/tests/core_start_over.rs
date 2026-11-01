//! Start over: while a take records or is paused, the item's Start over (or its key) tells the app the
//! take goes to the Trash once its files are finished, then stops it as Stop does. Refused, with nothing done, before
//! the gate and once the take is stopping. The next take starts with `StartAgain`, which counts down on the HUD as a
//! take the start key began.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::{Harness, Src, cfg};
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::run::core::Effect;
use klipolo_capture::shell::{HotkeyAction, MenuAction, ShellInput, ShellOutput, WorkerState};

const SOURCES: &[Src] = &[Src::Screen, Src::Camera, Src::Mic];

fn menu() -> ShellInput {
    ShellInput::Menu(MenuAction::StartOver)
}

fn started_over(h: &Harness) -> usize {
    (h.events().iter())
        .filter(|e| matches!(e, Event::StartOver))
        .count()
}

fn finalized(h: &Harness) -> usize {
    (h.effects().iter())
        .filter(|e| matches!(e, Effect::Finalize))
        .count()
}

#[test]
fn start_over_finishes_the_take_for_the_trash_from_recording_or_paused() {
    let recording = Harness::recording(SOURCES);
    let mut paused = Harness::recording(SOURCES);
    paused.command(Command::Pause);
    for (mut h, state, input) in [
        (recording, WorkerState::Recording, menu()),
        (
            paused,
            WorkerState::Paused,
            ShellInput::Hotkey(HotkeyAction::StartOver),
        ),
    ] {
        assert_eq!(h.state(), state);
        h.shell(input);
        assert_eq!(started_over(&h), 1, "{state:?}");
        assert_eq!(h.state(), WorkerState::Stopping, "{state:?}");
        assert_eq!(finalized(&h), 1, "{state:?}: finished as for Stop");
        // The app hears it before the take is stopping, so it never imports the take.
        let at = |want: &dyn Fn(&Event) -> bool| h.events().iter().position(want).unwrap();
        let over = at(&|e| matches!(e, Event::StartOver));
        let stopping = at(&|e| {
            matches!(
                e,
                Event::State {
                    state: WorkerState::Stopping,
                    ..
                }
            )
        });
        assert!(over < stopping, "{state:?}: {:?}", h.events());
        // A second press while it saves does nothing more.
        h.shell(menu());
        assert_eq!((started_over(&h), finalized(&h)), (1, 1), "{state:?}");
    }
}

#[test]
fn start_over_is_refused_before_the_gate_and_while_saving() {
    let mut armed = Harness::new(cfg(SOURCES, 3));
    armed.shell(menu());
    assert_eq!(armed.state(), WorkerState::Armed);
    let mut countdown = Harness::new(cfg(SOURCES, 3));
    countdown.command(Command::Start);
    countdown.feed(0.0, 1.5);
    countdown.shell(ShellInput::Hotkey(HotkeyAction::StartOver));
    assert_eq!(countdown.state(), WorkerState::Countdown);
    let mut saving = Harness::recording(SOURCES);
    saving.command(Command::Stop);
    saving.shell(menu());
    for (h, what) in [
        (&armed, "armed"),
        (&countdown, "counting down"),
        (&saving, "saving"),
    ] {
        assert_eq!(started_over(h), 0, "{what}");
        assert!(
            !h.effects().iter().any(|e| matches!(e, Effect::Exit(3))),
            "{what}: nothing ended"
        );
    }
    assert_eq!(finalized(&saving), 1, "saved once, as Stop asked");
}

/// Start over's next take: the editor stays hidden, so its countdown is the HUD's (`StatusView::by_key`) and the start
/// key answers the gate's question, as in a take the start key began.
#[test]
fn start_again_counts_down_on_the_hud() {
    let mut h = Harness::new(cfg(SOURCES, 3));
    h.command(Command::StartAgain);
    assert_eq!(h.state(), WorkerState::Countdown);
    let statuses: Vec<_> = (h.shell_out.iter())
        .filter_map(|o| match o {
            ShellOutput::Status(view) => Some(view),
            _ => None,
        })
        .collect();
    assert!(
        statuses
            .iter()
            .any(|v| v.state == WorkerState::Countdown && v.by_key),
        "{statuses:?}"
    );
    h.feed(0.0, 5.0);
    assert_eq!(h.state(), WorkerState::Recording);
    // A Record take's statuses never say so.
    let mut record = Harness::new(cfg(SOURCES, 3));
    record.command(Command::Start);
    assert!(!(record.shell_out.iter()).any(|o| matches!(o, ShellOutput::Status(v) if v.by_key)));
}

/// The camera bubble follows the take: shown while a live camera records, gone while paused or with the
/// camera lost, back on resume and on the camera's return; never in a take without the switch.
#[test]
fn the_camera_bubble_follows_the_take_and_its_camera() {
    use klipolo_capture::pipe::PipeEvent;
    let last = |h: &Harness| {
        (h.shell_out.iter().rev())
            .find_map(|o| match o {
                ShellOutput::Status(view) => Some(view.bubble),
                _ => None,
            })
            .expect("a status")
    };
    let mut on = cfg(SOURCES, 3);
    on.camera_bubble = true;
    let mut h = Harness::recording_cfg(on);
    h.feed_for(1.5);
    assert!(last(&h), "recording with a live camera");
    h.command(Command::Pause);
    assert!(!last(&h), "paused");
    h.command(Command::Resume);
    assert!(last(&h), "resumed");
    h.pipe_event("camera", PipeEvent::DeviceLost);
    h.feed_for(1.5);
    assert!(!last(&h), "the camera is lost");
    h.pipe_event("camera", PipeEvent::DeviceBack);
    h.feed_for(3.5);
    assert!(last(&h), "the camera is back");
    h.command(Command::Stop);
    assert!(!last(&h), "stopping");
    let mut off = Harness::recording(SOURCES);
    off.feed_for(1.5);
    assert!(!last(&off), "the switch is off");
}
