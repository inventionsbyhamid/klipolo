//! The cursor's failure paths: a stalled core never mistakes queued beats for silence, what the
//! source says before the gate or during a pause isn't lost, a source that fails before the gate burns in once
//! the take records, buttons still down are released at a pause and a burn-in, and the place sent before the
//! gate or during a pause is the cursor's from the gate or the resume. On the synchronous harness.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::Harness;
use klipolo_capture::cursor::CursorEvent as E;
use klipolo_capture::journal::Entry;
use klipolo_capture::preflight::cursor::CursorPlan;
use klipolo_capture::protocol::Command;
use klipolo_capture::run::core::PipeCmd;
use klipolo_model::cursor_style::CursorCapture;

fn ns(h: &Harness, at_s: f64) -> u64 {
    h.base + (at_s * 1e9) as u64
}

fn beat(h: &Harness, at_s: f64) -> E {
    E::Beat {
        host_ns: ns(h, at_s),
        x: 0.5,
        y: 0.5,
    }
}

fn burned_at(h: &Harness) -> Option<i64> {
    h.journal().iter().find_map(|e| match e {
        Entry::CursorBurned { at_us } => Some(*at_us),
        _ => None,
    })
}

#[test]
fn a_stalled_core_with_beats_queued_never_burns_the_cursor_in() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    for t in [0.5, 1.0, 1.5, 2.0] {
        h.cursor(t, beat(&h, t));
        h.run_to(t + 0.5);
    }
    // The core stalls for 1.6 s: its first input after the stall is a poll, with the source's healthy beats queued
    // behind it.
    h.poll_at(4.1);
    for t in [2.5, 3.0, 3.5, 4.0] {
        h.cursor(t, beat(&h, t));
    }
    for t in [4.5, 5.0, 5.5, 6.0] {
        h.cursor(t, beat(&h, t));
        h.run_to(t + 0.5);
    }
    assert!(
        !h.pipe_cmds("screen").contains(&PipeCmd::CursorInFrames),
        "{:?}",
        h.pipe_cmds("screen")
    );
    assert!(
        !h.journal()
            .iter()
            .any(|e| matches!(e, Entry::CursorBurnDecided { .. }))
    );
}

#[test]
fn a_shape_visibility_and_area_from_the_countdown_or_a_pause_are_kept() {
    // The source starts at Record and sends its current shape (hidden while typing) before the gate.
    let early = [
        E::NewShape {
            host_ns: 0,
            key: 5,
            image: common::shape(16, 16),
        },
        E::Visible {
            host_ns: 0,
            on: false,
        },
    ];
    let mut h = Harness::recording_cursor_after(CursorPlan::Separate, &early);
    h.cursor(
        1.0,
        E::Shape {
            host_ns: ns(&h, 1.0),
            key: 5,
        },
    );
    h.cursor(
        1.5,
        E::Visible {
            host_ns: ns(&h, 1.5),
            on: true,
        },
    );
    h.run_to(2.0);
    // A new shape and a resized area while paused.
    h.command(Command::Pause);
    h.cursor(
        2.5,
        E::NewShape {
            host_ns: ns(&h, 2.5),
            key: 6,
            image: common::shape(8, 8),
        },
    );
    h.cursor(
        2.6,
        E::Area {
            host_ns: ns(&h, 2.6),
            w_pts: 1280.0,
            h_pts: 800.0,
        },
    );
    h.run_to(3.0);
    h.command(Command::Resume);
    h.run_to(3.5);
    let t = h.cursor_track();
    let ids: Vec<u16> = t.shapes().iter().map(|s| s.id).collect();
    assert_eq!(ids, [0, 1], "both defined");
    assert!(h.files.contains_key("cursors/0.png") && h.files.contains_key("cursors/1.png"));
    assert_eq!(
        t.shape_at(500_000).map(|s| s.id),
        Some(0),
        "the countdown's shape from the gate, not the Arrow"
    );
    assert_eq!(t.shape_at(1_200_000).map(|s| s.id), Some(0));
    assert!(!t.visible_at(500_000) && t.visible_at(1_600_000));
    // Session time leaves the pause out: the take resumes at 2.0 s.
    assert_eq!(
        t.shape_at(2_100_000).map(|s| s.id),
        Some(1),
        "the pause's shape from the resume"
    );
    assert_eq!(t.area_at(2_100_000), (1280.0, 800.0));
    assert!(!t.notes().lost_any(), "{:?}", t.notes());
}

#[test]
fn the_place_sent_before_the_gate_and_during_a_pause_is_the_cursors_from_the_gate_and_the_resume() {
    // The source starts at Record and sends where the pointer is during the countdown, then rests (no beat yet).
    let early = [E::Move {
        host_ns: 0,
        x: 0.3,
        y: 0.4,
    }];
    let mut h = Harness::recording_cursor_after(CursorPlan::Separate, &early);
    h.run_to(0.3);
    let near = |p: (f32, f32), q: (f32, f32)| (p.0 - q.0).abs() < 1e-4 && (p.1 - q.1).abs() < 1e-4;
    let at_gate = h.cursor_track().position(0);
    assert!(at_gate.is_some_and(|p| near(p, (0.3, 0.4))), "{at_gate:?}");
    // It moves while paused, then rests: the take resumes (at 0.3 s of session time) where it is.
    h.command(Command::Pause);
    let host_ns = ns(&h, 0.5);
    h.cursor(
        0.5,
        E::Move {
            host_ns,
            x: 0.6,
            y: 0.7,
        },
    );
    h.command(Command::Resume);
    h.run_to(1.0);
    let at_resume = h.cursor_track().position(310_000);
    assert!(
        at_resume.is_some_and(|p| near(p, (0.6, 0.7))),
        "{at_resume:?}"
    );
}

#[test]
fn a_source_that_fails_before_the_gate_burns_in_once_the_take_records() {
    let mut h = Harness::recording_cursor_after(CursorPlan::Separate, &[E::Failed]);
    h.run_to(0.3);
    assert_eq!(
        h.pipe_cmds("screen"),
        [PipeCmd::CursorInFrames],
        "decided after the gate"
    );
    // The screen can't show the cursor live: it heals with a restart that has it.
    h.ack_cursor_in_frames(false);
    assert!(h.pipe_cmds("screen").contains(&PipeCmd::Restart));
    h.run_to(2.0);
    let second = h.run_offset_us("screen", 2);
    assert_eq!(burned_at(&h), Some(second));
    let capture = h.manifest().cursor.unwrap().capture;
    assert_eq!(
        capture,
        CursorCapture::BurnedFrom { at_us: second },
        "never from 0 over frames without it"
    );
    assert!(
        h.note_codes()
            .contains(&"capture.note.cursorBurned".to_string())
    );
}

#[test]
fn a_press_still_down_is_released_at_a_pause_and_at_a_burn_in() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    let down = |h: &Harness, t, btn| E::Button {
        host_ns: ns(h, t),
        btn,
        down: true,
        x: 0.5,
        y: 0.5,
    };
    h.cursor(1.0, down(&h, 1.0, 0));
    h.run_to(1.2);
    h.command(Command::Pause);
    h.run_to(2.0);
    h.command(Command::Resume);
    // Session time leaves the pause out: 1.2 s is where the take paused. Then a second press, and silence.
    h.cursor(2.3, down(&h, 2.3, 1));
    h.run_to(4.0);
    let presses = h.cursor_track().presses().to_vec();
    assert_eq!(presses.len(), 2, "{presses:?}");
    assert_eq!(
        (presses[0].down, presses[0].up),
        (1_000_000, 1_200_000),
        "released at the pause"
    );
    let decided = h.journal().iter().find_map(|e| match e {
        Entry::CursorBurnDecided { at_us } => Some(*at_us),
        _ => None,
    });
    assert_eq!(Some(presses[1].up), decided, "released at the burn-in");
}

#[test]
fn presses_of_one_button_that_arrive_out_of_order_are_sorted() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    let btn = |h: &Harness, t, down| E::Button {
        host_ns: ns(h, t),
        btn: 0,
        down,
        x: 0.5,
        y: 0.5,
    };
    // A later click is delivered before an earlier one (two threads), all inside the 50 ms window.
    h.cursor(1.000, btn(&h, 1.000, true));
    h.cursor(1.021, btn(&h, 1.020, false));
    h.cursor(1.022, btn(&h, 0.980, true));
    h.cursor(1.023, btn(&h, 0.990, false));
    h.run_to(2.0);
    let t = h.cursor_track();
    assert_eq!(t.clicks().count(), 2, "both kept");
    assert_eq!(t.skipped(), 0);
}

#[test]
fn a_switch_to_sampled_clicks_before_the_gate_or_in_a_pause_is_written_at_the_gate_or_the_resume() {
    // A monitor that couldn't install switches as its source starts, in the countdown.
    let early = [E::ClicksSampled { host_ns: 0 }];
    let mut h = Harness::recording_cursor_after(CursorPlan::Separate, &early);
    h.run_to(0.5);
    let sampled = klipolo_cursor::ClickSource::Sampled;
    assert_eq!(h.cursor_track().clicks_at(0), sampled, "from the gate");
    assert!(
        h.note_codes()
            .contains(&"capture.note.cursorSampled".into())
    );
    h.command(Command::Stop);
    h.run_to(1.5);
    assert_eq!(h.manifest().cursor.unwrap().clicks, sampled);
    // The self-check's switch inside a pause shows from the resume.
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    h.run_to(1.0);
    h.command(Command::Pause);
    h.cursor(
        1.1,
        E::ClicksSampled {
            host_ns: ns(&h, 1.1),
        },
    );
    h.run_to(1.5);
    h.command(Command::Resume);
    h.run_to(2.0);
    let t = h.cursor_track();
    assert_eq!(t.clicks_at(900_000), klipolo_cursor::ClickSource::Events);
    assert_eq!(t.clicks_at(1_100_000), sampled, "from the resume at 1.0 s");
}
