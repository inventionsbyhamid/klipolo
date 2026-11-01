//! A system alert on the screen while a take starts (`SessionCore::system_alert`): macOS's own questions, such as the
//! one a take's first stream can raise about the private window picker, hold the countdown while they are up, and the
//! take starts a whole countdown after the person answered, so the question is never in the recording.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::{Harness, Src, cfg};
use klipolo_capture::english::english;
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::run::core::Effect;
use klipolo_capture::shell::{ShellOutput, WorkerState};
use klipolo_model::coded::CodedError;

/// What the take said about the system's alert, in order: up, then gone.
fn alerts(h: &Harness) -> Vec<bool> {
    (h.events().iter())
        .filter_map(|e| match e {
            Event::SystemAlert { up } => Some(*up),
            _ => None,
        })
        .collect()
}

/// The countdown's seconds as it said them, in order.
fn counts(events: &[Event]) -> Vec<u32> {
    (events.iter())
        .filter_map(|e| match e {
            Event::Countdown { left_s } => Some(*left_s),
            _ => None,
        })
        .collect()
}

/// What the HUD's last status said held the countdown.
fn held(h: &Harness) -> Option<String> {
    (h.shell_out.iter().rev())
        .find_map(|o| match o {
            ShellOutput::Status(view) => Some(view.held.clone()),
            _ => None,
        })
        .flatten()
}

fn alert_words() -> String {
    english(&CodedError::new("capture.gate.systemAlert"))
}

#[test]
fn an_alert_holds_the_countdown_and_its_answer_starts_it_again_from_the_top() {
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Mic], 3));
    h.command(Command::Start);
    assert!(h.core.watches_system_alerts());
    h.feed(0.0, 2.5);
    h.system_alert(true);
    h.feed(2.5, 20.0);
    assert_eq!(h.state(), WorkerState::Countdown, "held while it is up");
    assert!(h.file_keys().is_empty(), "nothing is written meanwhile");
    assert_eq!(held(&h), Some(alert_words()), "the HUD says why");
    let before = h.events().len();
    h.system_alert(false);
    assert_eq!(held(&h), None);
    assert!(
        matches!(
            h.events()[before..],
            [
                Event::Countdown { left_s: 3 },
                Event::SystemAlert { up: false }
            ]
        ),
        "the count from the top is said before the alert's end: {:?}",
        &h.events()[before..]
    );
    h.feed(20.0, 22.9);
    assert_eq!(
        h.state(),
        WorkerState::Countdown,
        "a whole countdown after the answer"
    );
    h.feed(22.9, 23.5);
    assert_eq!(h.state(), WorkerState::Recording);
    let t0 = h.gate_s();
    assert!((23.0..23.1).contains(&t0), "{t0}");
    assert_eq!(alerts(&h), [true, false]);
    assert_eq!(counts(&h.events()[before..]), [3, 2, 1, 0]);
    assert!(!h.core.watches_system_alerts(), "only until the gate");
}

#[test]
fn an_alert_up_as_the_countdown_starts_holds_it_at_its_top() {
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Mic], 3));
    h.command(Command::Start);
    h.system_alert(true);
    h.feed(0.0, 10.0);
    assert_eq!(h.state(), WorkerState::Countdown);
    let said = counts(h.events());
    assert!(
        !said.is_empty() && said.iter().all(|&s| s == 3),
        "it never counted down: {said:?}"
    );
    h.system_alert(false);
    h.feed(10.0, 13.5);
    assert_eq!(h.state(), WorkerState::Recording);
    let t0 = h.gate_s();
    assert!((13.0..13.1).contains(&t0), "{t0}");
}

#[test]
fn an_alert_said_again_and_again_is_one_hold() {
    let mut h = Harness::new(cfg(&[Src::Screen], 3));
    h.command(Command::Start);
    for at in [0.5, 1.0, 1.5] {
        h.feed(at - 0.5, at);
        h.system_alert(true);
    }
    h.feed(1.5, 5.0);
    h.system_alert(false);
    h.system_alert(false);
    assert_eq!(alerts(&h), [true, false], "each change is said once");
}

#[test]
fn an_alert_before_the_start_or_after_the_gate_changes_nothing() {
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Mic], 3));
    assert!(!h.core.watches_system_alerts(), "armed: not yet");
    h.system_alert(true);
    assert!(alerts(&h).is_empty());
    h.system_alert(false);
    h.start_recording();
    let t0 = h.gate_s();
    assert!(
        (3.0..3.2).contains(&t0),
        "{t0}: one said while armed holds nothing"
    );
    h.system_alert(true);
    h.feed_for(2.0);
    assert_eq!(h.state(), WorkerState::Recording);
    assert!(alerts(&h).is_empty(), "nothing to hold once it records");
}

#[test]
fn cancel_while_an_alert_holds_the_countdown_leaves_no_take() {
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Mic], 3));
    h.command(Command::Start);
    h.system_alert(true);
    h.feed(0.0, 6.0);
    h.command(Command::Cancel);
    assert!(h.file_keys().is_empty());
    assert!(h.effects().iter().any(|e| matches!(e, Effect::Exit(3))));
    assert!(!h.core.watches_system_alerts());
}
