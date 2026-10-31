//! The cursor writer's finer rules: the held place before a move after a
//! still and the rest point, an area not known at the header, a time ahead of its arrival,
//! a press still down at the end, the heal when the live switch is refused, a full
//! shape store; and the plan (beside `cursor.rs`; the disk budget is `cursor_disk.rs`). On the
//! synchronous harness.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::Harness;
use klipolo_capture::cursor::CursorEvent as E;
use klipolo_capture::journal::Entry;
use klipolo_capture::preflight::cursor::CursorPlan;
use klipolo_capture::protocol::Command;
use klipolo_capture::run::core::PipeCmd;
use klipolo_cursor::ClickSource;

fn ns(h: &Harness, at_s: f64) -> u64 {
    h.base + (at_s * 1e9) as u64
}

fn mv(h: &Harness, at_s: f64, x: f32) -> E {
    E::Move {
        host_ns: ns(h, at_s),
        x,
        y: 0.5,
    }
}

#[test]
fn a_move_after_a_still_holds_the_place_until_it_starts_and_a_rest_point_is_kept() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    // 1,000 Hz for 30 ms (thinned to 4 ms), coming to rest at x 0.33 at 1.029 s.
    for i in 0..30 {
        let t = 1.0 + 0.001 * f64::from(i);
        h.cursor(t, mv(&h, t, 0.30 + 0.001 * i as f32));
    }
    // Still for 0.4 s with no beat, then a far move.
    h.cursor(1.43, mv(&h, 1.43, 0.9));
    h.run_to(2.0);
    let t = h.cursor_track();
    let at = |s: f64| t.position((s * 1e6) as i64).unwrap().0;
    assert!(
        (at(1.029) - 0.329).abs() < 1e-4,
        "the rest point: {}",
        at(1.029)
    );
    assert!(
        (at(1.40) - 0.329).abs() < 1e-4,
        "held, not drifting towards the move: {}",
        at(1.40)
    );
    assert!((at(1.43) - 0.9).abs() < 1e-4);
    assert!(!t.notes().lost_any(), "{:?}", t.notes());
}

#[test]
fn an_area_not_known_at_the_header_is_unknown_until_its_line_and_never_refused() {
    let mut h = Harness::recording_cursor_with(CursorPlan::Separate, None);
    h.cursor(
        0.5,
        E::Area {
            host_ns: ns(&h, 0.5),
            w_pts: 1440.0,
            h_pts: 900.0,
        },
    );
    h.cursor(0.6, mv(&h, 0.6, 0.5));
    h.run_to(1.0);
    let t = h.cursor_track();
    assert_eq!(t.header().unwrap().area_pts, (0.0, 0.0));
    assert_eq!(t.area_at(600_000), (1440.0, 900.0));
    assert!(!t.notes().lost_any(), "{:?}", t.notes());
}

#[test]
fn a_time_ahead_of_its_arrival_is_written_at_its_arrival() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    // A bogus stamp 10 s in the future would silence every later move (each kind is ordered on its own).
    let ahead = E::Move {
        host_ns: ns(&h, 11.0),
        x: 0.2,
        y: 0.5,
    };
    h.cursor(1.0, ahead);
    h.cursor(1.5, mv(&h, 1.5, 0.4));
    h.run_to(2.0);
    let t = h.cursor_track();
    let times: Vec<i64> = t.samples(i64::MIN, i64::MAX).map(|s| s.0).collect();
    assert_eq!(times.first(), Some(&1_000_000));
    assert_eq!(times.last(), Some(&1_500_000));
    assert_eq!(t.skipped(), 0);
}

#[test]
fn a_press_still_down_is_released_at_stop() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    h.cursor(
        1.0,
        E::Button {
            host_ns: ns(&h, 1.0),
            btn: 0,
            down: true,
            x: 0.5,
            y: 0.5,
        },
    );
    h.run_to(2.0);
    h.command(Command::Stop);
    let presses = h.cursor_track().presses().to_vec();
    assert_eq!(presses.len(), 1);
    assert_eq!(
        (presses[0].down, presses[0].up),
        (1_000_000, 2_000_000),
        "not held to the end of time"
    );
}

#[test]
fn a_failed_source_with_a_refused_live_switch_heals_and_burns_in_at_the_new_run() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    h.cursor(1.0, mv(&h, 1.0, 0.5));
    h.cursor(1.2, E::Failed);
    h.run_to(1.3);
    assert_eq!(h.pipe_cmds("screen"), [PipeCmd::CursorInFrames], "at once");
    let decided = h
        .journal()
        .iter()
        .any(|e| matches!(e, Entry::CursorBurnDecided { .. }));
    assert!(decided);
    h.ack_cursor_in_frames(false);
    let cmds = h.pipe_cmds("screen");
    assert_eq!(
        cmds[..2],
        [PipeCmd::CursorInFrames, PipeCmd::Restart],
        "{cmds:?}"
    );
    assert_eq!(h.screen_spec().cursor, CursorPlan::Burned);
    h.run_to(3.0);
    assert_eq!(h.run_count("screen"), 2);
    let burned = h.journal().iter().find_map(|e| match e {
        Entry::CursorBurned { at_us } => Some(*at_us),
        _ => None,
    });
    assert_eq!(
        burned,
        Some(h.run_offset_us("screen", 2)),
        "at the healed run's first frame"
    );
    assert!(!h.cursor_track().separate_at(burned.unwrap()));
}

#[test]
fn a_full_shape_store_draws_new_shapes_as_the_arrow_and_says_so_once() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    for key in 0..70u64 {
        let t = 0.5 + 0.01 * key as f64;
        let image = common::shape(4, 4);
        h.cursor(
            t,
            E::NewShape {
                host_ns: ns(&h, t),
                key,
                image,
            },
        );
    }
    h.run_to(2.0);
    let t = h.cursor_track();
    assert_eq!(t.shapes().len(), klipolo_cursor::MAX_SHAPES);
    assert!(
        t.shape_at(1_150_000).is_none(),
        "the 66th draws as the Arrow look"
    );
    let notes = h.note_codes();
    assert_eq!(
        notes
            .iter()
            .filter(|c| *c == "capture.note.cursorShapes")
            .count(),
        1
    );
}

#[test]
fn a_source_past_its_shape_limit_keeps_the_last_shape_and_says_so_once() {
    // macOS's source stops sending new shapes past the limit (an animated cursor) and says so once.
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    let (key, image) = (7, common::shape(16, 16));
    h.cursor(
        1.0,
        E::NewShape {
            host_ns: ns(&h, 1.0),
            key,
            image,
        },
    );
    for t in [1.5, 2.0] {
        h.cursor(t, E::ShapesFull { host_ns: ns(&h, t) });
    }
    h.run_to(2.5);
    let t = h.cursor_track();
    assert_eq!(
        t.shape_at(2_200_000).map(|s| s.id),
        Some(0),
        "the last defined shape shows on"
    );
    let notes = h.note_codes();
    let full = notes
        .iter()
        .filter(|c| *c == "capture.note.cursorShapes")
        .count();
    assert_eq!(full, 1);
}

#[test]
fn games_get_the_cursor_only_as_their_profile_asks_and_screens_stay_burned_until_the_gate() {
    use klipolo_capture::preflight::cursor::{SEPARATE_ENABLED, cursor_plan};
    use klipolo_model::project::RecordingKind;
    let caps = klipolo_capture::CursorCaps {
        hide: true,
        positions: true,
        shapes: true,
        clicks: ClickSource::Events,
    };
    assert_eq!(
        cursor_plan(RecordingKind::Gameplay, &caps, false),
        CursorPlan::None
    );
    assert_eq!(
        cursor_plan(RecordingKind::Gameplay, &caps, true),
        CursorPlan::Burned
    );
    assert_eq!(
        cursor_plan(RecordingKind::Screen, &caps, true),
        if SEPARATE_ENABLED {
            CursorPlan::Separate
        } else {
            CursorPlan::Burned
        }
    );
    let cant_hide = klipolo_capture::CursorCaps {
        hide: false,
        ..caps
    };
    assert_eq!(
        cursor_plan(RecordingKind::Screen, &cant_hide, true),
        CursorPlan::Burned
    );
    assert_eq!(
        cursor_plan(RecordingKind::Camera, &caps, true),
        CursorPlan::None
    );
}

#[test]
fn a_place_held_from_a_pause_is_never_restated_after_newer_moves() {
    // A move stamped inside a pause arrives after its resume (the delivery lag), so it is held;
    // the moves after it are timed. A later pause with no move in it ends without bringing the old place back.
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    h.run_to(1.0);
    h.command(Command::Pause);
    h.run_to(1.1);
    h.command(Command::Resume);
    h.cursor(1.12, mv(&h, 1.05, 0.9));
    for i in 0..40 {
        let t = 1.2 + 0.02 * f64::from(i);
        h.cursor(t, mv(&h, t, 0.2));
    }
    h.run_to(2.0);
    h.command(Command::Pause);
    h.run_to(2.1);
    h.command(Command::Resume);
    h.run_to(2.6);
    // Session time leaves the two pauses out: the second resume is at 1.9 s.
    let at = h.cursor_track().position(2_000_000).map(|p| p.0);
    assert!(at.is_some_and(|x| (x - 0.2).abs() < 1e-4), "{at:?}");
}
