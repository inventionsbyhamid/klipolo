//! The take's cursor: format 2 written through the reorder window, shapes, the watchdog's burn-in, a switch to
//! sampled clicks, a pause, and resume after a crash (the plan and the disk budget are in `cursor_more.rs`). On the
//! synchronous harness and its fake clock; read back through `klipolo_cursor` as the engine and chat read
//! it.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::{Harness, Src};
use klipolo_capture::cursor::CursorEvent as E;
use klipolo_capture::journal::Entry;
use klipolo_capture::preflight::cursor::CursorPlan;
use klipolo_capture::run::core::PipeCmd;
use klipolo_cursor::format::Capture;
use klipolo_cursor::{ClickSource, CursorTrack, Limits};

const S: f64 = 1.0;
fn ns(h: &Harness, at_s: f64) -> u64 {
    h.base + (at_s * 1e9) as u64
}

#[test]
fn the_cursor_file_is_format_two_with_its_header_positions_and_presses() {
    // The harness's screen sends a point a frame (30 fps), pressed for one frame every 2 s.
    let mut h = Harness::recording(&[Src::Screen]);
    h.feed_for(10.0);
    // As Stop leaves it: the gathered lines appended.
    let files = h.stopped_files();
    let bytes = &files["cursor.jsonl"];
    let track = CursorTrack::read(&bytes[..], Limits::for_file_len(bytes.len() as u64));
    let head = track.header().expect("a header line first");
    assert_eq!(
        (head.capture, head.clicks),
        (Capture::Burned, ClickSource::Sampled)
    );
    assert!(head.area_pts.0 > 0.0 && head.area_pts.1 > 0.0, "{head:?}");
    assert!(track.len() >= 290, "a position a frame: {}", track.len());
    assert!(
        !track.notes().lost_any(),
        "nothing out of order: {:?}",
        track.notes()
    );
    assert_eq!(track.notes().headers, 1, "one header");
    let clicks: Vec<_> = track.clicks().collect();
    assert!((4..=6).contains(&clicks.len()), "{clicks:?}");
    assert!(
        clicks
            .iter()
            .all(|c| c.up > c.down && c.up - c.down < 100_000)
    );
}

#[test]
fn a_separate_take_writes_a_header_moves_presses_and_each_shape_once() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    for i in 0..60 {
        let t = 0.1 * i as f64;
        h.cursor(
            t,
            E::Move {
                host_ns: ns(&h, t),
                x: 0.5,
                y: 0.4,
            },
        );
    }
    h.cursor(
        1.0,
        E::NewShape {
            host_ns: ns(&h, 1.0),
            key: 7,
            image: common::shape(32, 32),
        },
    );
    h.cursor(
        2.0,
        E::Shape {
            host_ns: ns(&h, 2.0),
            key: 7,
        },
    );
    h.cursor(
        3.0,
        E::Button {
            host_ns: ns(&h, 3.0),
            btn: 0,
            down: true,
            x: 0.5,
            y: 0.4,
        },
    );
    h.cursor(
        3.1,
        E::Button {
            host_ns: ns(&h, 3.1),
            btn: 0,
            down: false,
            x: 0.5,
            y: 0.4,
        },
    );
    h.run_to(6.0 * S);
    let t = h.cursor_track();
    assert_eq!(t.header().unwrap().capture, klipolo_cursor::Capture::Separate);
    assert_eq!((t.clicks().count(), t.shapes().len()), (1, 1));
    assert!(h.files.contains_key("cursors/0.png"));
    assert!(!t.notes().lost_any(), "{:?}", t.notes());
}

#[test]
fn a_button_that_arrives_after_a_later_beat_is_kept() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    h.cursor(
        1.000,
        E::Beat {
            host_ns: ns(&h, 1.000),
            x: 0.5,
            y: 0.5,
        },
    );
    // The monitor's button, stamped 15 ms earlier by the hardware, is delivered after the beat.
    h.cursor(
        1.001,
        E::Button {
            host_ns: ns(&h, 0.985),
            btn: 0,
            down: true,
            x: 0.5,
            y: 0.5,
        },
    );
    h.cursor(
        1.002,
        E::Button {
            host_ns: ns(&h, 0.990),
            btn: 0,
            down: false,
            x: 0.5,
            y: 0.5,
        },
    );
    h.run_to(2.0);
    assert_eq!(h.cursor_track().clicks().count(), 1);
    assert_eq!(h.cursor_track().skipped(), 0, "the writer sorted the batch");
}

#[test]
fn a_silent_cursor_source_burns_the_cursor_in_within_a_second_and_a_half() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    h.cursor(
        3.0,
        E::Beat {
            host_ns: ns(&h, 3.0),
            x: 0.5,
            y: 0.5,
        },
    );
    h.run_to(4.4);
    assert!(
        h.pipe_cmds("screen").is_empty(),
        "1.4 s of silence: not yet"
    );
    h.run_to(4.6);
    assert_eq!(h.pipe_cmds("screen"), [PipeCmd::CursorInFrames]);
    h.ack_cursor_in_frames(true);
    let burned = h
        .journal()
        .iter()
        .find_map(|e| match e {
            Entry::CursorBurned { at_us } => Some(*at_us),
            _ => None,
        })
        .unwrap();
    assert!(!h.cursor_track().separate_at(burned));
    assert!(
        h.note_codes()
            .contains(&"capture.note.cursorBurned".to_string())
    );
    // Until the stream confirmed it, the file says the cursor was separate (the engine holds the last one there).
    assert!(h.cursor_track().separate_at(burned - 1));
    assert_eq!(
        h.manifest().cursor.unwrap().capture,
        klipolo_model::cursor_style::CursorCapture::BurnedFrom { at_us: burned }
    );
}

#[test]
fn a_switch_to_sampled_clicks_is_carried_and_noted() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    h.cursor(
        2.0,
        E::ClicksSampled {
            host_ns: ns(&h, 2.0),
        },
    );
    h.run_to(3.0);
    let t = h.cursor_track();
    assert_eq!(
        (t.clicks_at(1.0e6 as i64), t.clicks_weakest()),
        (ClickSource::Events, ClickSource::Sampled)
    );
    assert!(
        h.note_codes()
            .contains(&"capture.note.cursorSampled".to_string())
    );
    h.command(klipolo_capture::protocol::Command::Stop);
    h.run_to(5.0);
    assert_eq!(h.manifest().cursor.unwrap().clicks, ClickSource::Sampled);
}

#[test]
fn a_paused_take_writes_no_cursor_lines_and_never_burns_in_while_paused() {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    h.command(klipolo_capture::protocol::Command::Pause);
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
    h.run_to(8.0);
    // (The screen was told to pause; never to show the cursor.)
    assert!(!h.pipe_cmds("screen").contains(&PipeCmd::CursorInFrames));
    assert_eq!(h.cursor_track().presses().len(), 0);
}

#[test]
fn a_take_resumed_after_a_burn_in_keeps_the_cursor_in_its_frames() {
    let burned = common::harness::cursor::burned_take(4.0); // a take that burned in at 4 s, then its worker was killed
    let h = Harness::resume_from(&burned, 6.0);
    assert_eq!(
        h.screen_spec().cursor,
        CursorPlan::Burned,
        "the resumed screen run shows the cursor"
    );
    assert!(h.core.cursor_watch().burned());
    let t = h.cursor_track();
    assert_eq!(t.notes().headers, 1, "no second header");
    assert!(
        t.last_times().burned.is_some(),
        "the x line the burn-in wrote"
    );
}

#[test]
fn a_resumed_take_continues_its_shape_ids() {
    let mut h = Harness::resume_from(&common::harness::cursor::take_with_shapes(2), 3.0);
    h.cursor(
        3.5,
        E::NewShape {
            host_ns: ns(&h, 3.5),
            key: 99,
            image: common::shape(16, 16),
        },
    );
    h.run_to(4.0);
    assert!(h.files.contains_key("cursors/2.png") && h.cursor_track().shapes().len() == 3);
    assert_eq!(h.cursor_track().notes().headers, 1);
}
