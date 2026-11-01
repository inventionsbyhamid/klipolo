//! A resumed take's cursor: resume never fails on the cursor file (it keeps what
//! it can and goes on), a burn-in the stream hadn't answered completes at the resumed run's first frame, a
//! `CursorBurned` whose `x` line didn't reach the file gets it at the opening, and a cursor hidden at the crash shows
//! again when the source says so.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::Harness;
use klipolo_capture::cursor::CursorEvent as E;
use klipolo_capture::io::OsIo;
use klipolo_capture::journal::Entry;
use klipolo_capture::preflight::cursor::CursorPlan;
use klipolo_capture::run::resume::resume_with;
use klipolo_capture::session::CURSOR;
use klipolo_model::time::Us;

fn burned_at(journal: &[Entry]) -> Option<i64> {
    journal.iter().find_map(|e| match e {
        Entry::CursorBurned { at_us } => Some(*at_us),
        _ => None,
    })
}

#[test]
fn resume_goes_on_when_the_cursor_file_cant_be_read() {
    let root = tempfile::tempdir().unwrap();
    let dir = common::take::write_take(root.path(), "take", common::take::Take::default()).dir;
    // Something that isn't a file where the cursor file goes (any error opening it).
    std::fs::create_dir(dir.cursor()).unwrap();
    let (_lock, from) =
        resume_with(&dir, &OsIo, 0).expect("the take goes on without its cursor's past");
    assert!(from.cursor.writer.is_none());
    assert!(from.clock.started());
}

#[test]
fn a_burn_in_the_stream_hadnt_answered_completes_at_the_resumed_runs_first_frame() {
    let files = common::harness::cursor::decided_take(4.0);
    let mut h = Harness::resume_from(&files, 6.0);
    assert_eq!(h.screen_spec().cursor, CursorPlan::Burned);
    h.run_to(8.0);
    let run = h.run_offset_us("screen", 2);
    assert_eq!(
        burned_at(h.journal()),
        Some(run),
        "at the resumed run's first frame"
    );
    let t = h.cursor_track();
    assert_eq!(t.last_times().burned, Some(run), "its x line");
    assert!(!t.separate_at(run));
    assert_eq!(t.notes().headers, 1);
}

#[test]
fn a_burn_in_whose_x_line_didnt_land_gets_it_at_the_opening() {
    let mut files = common::harness::cursor::burned_take(4.0);
    let journal = String::from_utf8(files["journal.jsonl"].clone()).unwrap();
    let at = journal
        .lines()
        .filter_map(|l| serde_json::from_str::<Entry>(l).ok())
        .find_map(|e| match e {
            Entry::CursorBurned { at_us } => Some(at_us),
            _ => None,
        })
        .expect("the take burned in");
    // The journal's line reached the disk, the cursor file's `x` line didn't.
    let cursor = String::from_utf8(files[CURSOR].clone()).unwrap();
    let kept: String = cursor
        .lines()
        .filter(|l| !l.contains("\"k\":\"x\""))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_ne!(kept.len(), cursor.len());
    files.insert(CURSOR.into(), kept.into_bytes());
    let h = Harness::resume_from(&files, 6.0);
    let t = h.cursor_track();
    assert_eq!(t.last_times().burned, Some(at));
    assert!(t.separate_at(at - 1) && !t.separate_at(at));
}

#[test]
fn a_cursor_hidden_when_the_take_crashed_shows_again_when_the_source_says_so() {
    let ns = |h: &Harness, at_s: f64| h.base + (at_s * 1e9) as u64;
    let beat = |h: &Harness, at_s: f64| E::Beat {
        host_ns: ns(h, at_s),
        x: 0.3,
        y: 0.5,
    };
    // An app hides the cursor while the person types, and the worker crashes in that span: the file ends hidden.
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    h.cursor(0.1, beat(&h, 0.1));
    let host_ns = ns(&h, 1.0);
    h.cursor(1.0, E::Visible { host_ns, on: false });
    for t in [1.5, 2.0] {
        h.cursor(t, beat(&h, t));
    }
    h.run_to(2.5);
    let mut h = Harness::resume_from(&h.killed_files(), 3.5);
    assert!(
        !h.cursor_track().visible_at(Us::MAX - 1),
        "the file ends hidden"
    );
    // The resumed source shows it as the person moves the mouse.
    let host_ns = ns(&h, 4.0);
    h.cursor(4.0, E::Visible { host_ns, on: true });
    h.cursor(4.0, beat(&h, 4.0));
    h.run_to(4.5);
    let t = h.cursor_track();
    assert!(!t.visible_at(1_500_000));
    assert!(t.visible_at(Us::MAX - 1), "shown again");
}
