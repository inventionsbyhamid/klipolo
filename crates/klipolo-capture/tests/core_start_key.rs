//! The person's start key and the countdown's question. In a take the start key began (its editor
//! is hidden: "Start from my game"), the countdown holds on "{source} isn't sending anything yet." until it is
//! answered, and nothing is recorded meanwhile; the start key answers "Record without it", the item's Cancel ends it
//! with no take. In a take Record began, the screen asks, and the key stops it as ever.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::{Harness, Src, cfg};
use klipolo_capture::journal::Entry;
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::run::core::Effect;
use klipolo_capture::shell::{HotkeyAction, MenuAction, ShellInput, WorkerState};

fn key() -> ShellInput {
    ShellInput::Hotkey(HotkeyAction::StartStop)
}

/// A screen and a mic that sends only exact zeros (no real samples): the gate asks about the mic after 3 s.
fn take_with_a_silent_mic() -> Harness {
    let mut h = Harness::new(cfg(&[Src::Screen, Src::Mic], 3));
    h.mic_signal_zeros();
    h
}

fn asked_about_the_mic(h: &Harness) -> bool {
    (h.events().iter())
        .any(|e| matches!(e, Event::Gate { missing } if missing == &["mic".to_string()]))
}

fn gated(h: &Harness) -> bool {
    h.journal().iter().any(|e| matches!(e, Entry::Gate { .. }))
}

fn no_take(h: &Harness) -> bool {
    h.effects().iter().any(|e| matches!(e, Effect::Exit(3)))
}

#[test]
fn unanswered_the_question_holds_a_take_the_start_key_began_and_nothing_is_recorded() {
    let mut h = take_with_a_silent_mic();
    h.shell(key());
    h.feed(0.0, 120.0);
    assert!(asked_about_the_mic(&h));
    assert_eq!(h.state(), WorkerState::Countdown, "held for two minutes");
    assert!(!gated(&h), "no gate, so nothing is written");
    assert!(h.file_keys().is_empty());
}

#[test]
fn the_start_key_answers_record_without_it_and_the_take_records_the_rest() {
    let mut h = take_with_a_silent_mic();
    h.shell(key());
    h.feed(0.0, 6.0);
    assert!(asked_about_the_mic(&h));
    h.shell(key());
    h.feed(6.0, 10.0);
    assert_eq!(h.state(), WorkerState::Recording);
    assert!(gated(&h));
    assert!(!no_take(&h));
}

#[test]
fn the_items_cancel_answers_it_with_no_take() {
    let mut h = take_with_a_silent_mic();
    h.shell(key());
    h.feed(0.0, 6.0);
    h.shell(ShellInput::Menu(MenuAction::Stop));
    assert!(no_take(&h));
    assert!(!gated(&h));
}

#[test]
fn in_a_take_record_began_the_key_still_stops_it_during_the_question() {
    let mut h = take_with_a_silent_mic();
    h.command(Command::Start);
    h.feed(0.0, 6.0);
    assert!(asked_about_the_mic(&h));
    h.shell(key());
    assert!(no_take(&h));
    assert!(!gated(&h));
}
