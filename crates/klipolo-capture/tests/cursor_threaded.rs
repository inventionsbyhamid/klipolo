//! The cursor's fallback on threads (the only threaded cursor case): a screen whose pipe can't show the cursor
//! live heals with a restart that has it in its frames. Outcomes only, never wall-clock windows.
#![cfg(feature = "synthetic")]

mod common;

use common::harness::{Src, cfg, script_start_then_stop_after_ms};
use klipolo_capture::manifest::read_manifest;
use klipolo_capture::preflight::cursor::CursorPlan;
use klipolo_capture::run::threads::run_with;
use klipolo_capture::session::{SessionDir, SourceSpec, write_config};
use klipolo_capture::synthetic::{CursorScript, FakeBackend, Script};
use klipolo_model::cursor_style::CursorCapture;

#[test]
fn a_live_switch_that_fails_heals_with_the_cursor_in_the_frames() {
    let root = tempfile::tempdir().unwrap();
    let dir = SessionDir::create(root.path(), "take").unwrap();
    let mut config = cfg(&[Src::Screen], 0);
    if let SourceSpec::Screen(v) = &mut config.sources[0].spec {
        v.cursor = CursorPlan::Separate;
    }
    write_config(&dir, &config).unwrap();
    // The cursor's source goes silent 2 s after Record, and the screen can't show the cursor without a restart.
    let backend = FakeBackend::new(Script {
        cursor: Some(CursorScript::default().silent_after(2.0)),
        live_switch: false,
        ..Script::default()
    });
    let plans = backend.cursor_plans();
    let code = run_with(
        Box::new(backend),
        dir.clone(),
        false,
        script_start_then_stop_after_ms(7_000),
    );
    assert_eq!(code, 0);
    let plans = plans.lock().unwrap().clone();
    assert_eq!(
        plans,
        [CursorPlan::Separate, CursorPlan::Burned],
        "the screen's second run shows the cursor"
    );
    let m = read_manifest(&dir).unwrap();
    let cursor = m.cursor.expect("the take's cursor");
    let screen = m.sources.iter().find(|s| s.id == "screen").unwrap();
    assert_eq!(screen.runs.len(), 2, "{:?}", screen.runs);
    let (first, second) = (&screen.runs[0], &screen.runs[1]);
    // A file's length is in 90 kHz ticks, its last frame keeping the frame before's duration, so the first can end up to
    // a tick (11 µs) past the healed run's first frame; import ends it there (`import::runs::apart`).
    let gap = second.offset_us - (first.offset_us + first.duration_us);
    assert!((-12..=3_000_000).contains(&gap), "gap {gap} µs");
    assert_eq!(
        cursor.capture,
        CursorCapture::BurnedFrom {
            at_us: second.offset_us
        },
        "from the healed run's first frame"
    );
    assert!(
        m.notes
            .iter()
            .any(|n| n.code == "capture.note.cursorBurned")
    );
}
