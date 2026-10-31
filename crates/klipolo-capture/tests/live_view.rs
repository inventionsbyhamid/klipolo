//! The recorder's live view in its worker, with the fake backend: its frames reach the app until the
//! app's Stop; without Screen Recording it refuses and builds nothing; a failed start says why; a stream that ends by
//! itself says so, so the UI never keeps its last frame as live; and its picture follows the view's size as the app
//! says it. Nothing captures.
#![cfg(feature = "synthetic")]

#[path = "common/preview_io.rs"]
mod preview_io;

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use klipolo_capture::backend::{Permission, Target};
use klipolo_capture::live_view_size::ViewPixels;
use klipolo_capture::pipe::{PipeError, PipeEvent};
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::run::live_view::live_view_with;
use klipolo_capture::synthetic::{FAKE_DISPLAY, FakeBackend, Script};
use preview_io::{SharedOut, sent_after, sent_at};

/// The live view of the whole screen on `script`, the app's Stop sent `stop_ms` after it starts: its exit code, what
/// it told the app, and how many pipes the backend built.
fn view(script: Script, stop_ms: u64) -> (i32, Vec<Event>, u64) {
    let backend = FakeBackend::new(script);
    let opened = backend.opened();
    let out = SharedOut::default();
    let screen = Target::Display {
        id: "display-1".into(),
    };
    let input = sent_after(&Command::Stop, stop_ms);
    let code = live_view_with(
        Box::new(backend),
        &screen,
        None,
        input,
        Box::new(out.clone()),
    );
    (code, out.events(), opened.load(Ordering::SeqCst))
}

fn frames(events: &[Event]) -> usize {
    (events.iter())
        .filter(|e| matches!(e, Event::Preview { .. }))
        .count()
}

fn failed(events: &[Event]) -> Vec<&str> {
    (events.iter())
        .filter_map(|e| match e {
            Event::Failed { error } => Some(error.code.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn its_frames_reach_the_app_until_the_app_s_stop() {
    let (code, events, opened) = view(Script::default(), 1_000);
    assert_eq!(code, 0);
    assert_eq!(opened, 1);
    // About 10 a second, for 1 s.
    let n = frames(&events);
    assert!((8..=12).contains(&n), "{n} frames");
    assert!(failed(&events).is_empty(), "{events:?}");
}

#[test]
fn without_screen_recording_it_refuses_and_builds_nothing() {
    // Never a prompt: the permission is read first, and nothing else is called.
    for screen_permission in [
        Permission::NotAsked,
        Permission::Denied,
        Permission::Unknown,
    ] {
        let script = Script {
            screen_permission,
            ..Script::default()
        };
        let (code, events, opened) = view(script, 300);
        assert_eq!(code, 1);
        assert_eq!(opened, 0, "the backend's live view was never built");
        assert_eq!(failed(&events), ["capture.permission.screen"]);
        assert_eq!(frames(&events), 0);
    }
}

#[test]
fn a_start_that_fails_says_why() {
    // A computer without a live view (Windows and Linux for now) says so, and the UI shows nothing; a permission the
    // system refused at the start is Screen Recording's step.
    for (error, code) in [
        (PipeError::Unsupported, "capture.error.unsupported"),
        (PipeError::PermissionDenied, "capture.permission.screen"),
        (PipeError::NoDevice, "capture.error.noDevice"),
    ] {
        let script = Script {
            start_errors: BTreeMap::from([("liveView".to_string(), error)]),
            ..Script::default()
        };
        let (exit, events, _) = view(script, 300);
        assert_eq!(exit, 1);
        assert_eq!(failed(&events), [code]);
    }
}

#[test]
fn a_stream_that_ends_by_itself_says_so_and_sends_nothing_after() {
    // The stream ends 350 ms in, long before the app's Stop: its frames came, then the app hears why, and nothing more.
    let script = Script {
        events: vec![(350, "liveView".into(), PipeEvent::StoppedBySystem)],
        ..Script::default()
    };
    let started = Instant::now();
    let (code, events, _) = view(script, 5_000);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "it ended by itself"
    );
    assert_eq!(code, 1);
    assert!(frames(&events) >= 3, "{events:?}");
    assert_eq!(failed(&events), ["capture.liveView.unavailable"]);
    assert!(matches!(events.last(), Some(Event::Failed { .. })));
}

/// Each frame's size, in order.
fn sizes(events: &[Event]) -> Vec<(u32, u32)> {
    (events.iter())
        .filter_map(|e| match e {
            Event::Preview { width, height, .. } => Some((*width, *height)),
            _ => None,
        })
        .collect()
}

#[test]
fn its_picture_follows_the_view_s_size_and_goes_back_to_the_small_one() {
    // Started for a Large view of 1600 × 1000 pixels, then the window grows past 1080p, then the view goes back to
    // Small: the picture follows each, within 1080p and the display's own pixels, and so does its rate.
    assert_eq!(FAKE_DISPLAY, (2560, 1600));
    let backend = FakeBackend::new(Script::default());
    let out = SharedOut::default();
    let screen = Target::Display {
        id: "display-1".into(),
    };
    let big = |width, height| Some(ViewPixels { width, height });
    let input = sent_at(&[
        (
            400,
            Command::ViewSize {
                view: big(5120, 2880),
            },
        ),
        (800, Command::ViewSize { view: None }),
        (1_200, Command::Stop),
    ]);
    let code = live_view_with(
        Box::new(backend),
        &screen,
        big(1600, 1000),
        input,
        Box::new(out.clone()),
    );
    assert_eq!(code, 0);
    let mut sizes = sizes(&out.events());
    // 19 a second, then 16, then 10.
    let n = sizes.len();
    assert!((12..=24).contains(&n), "{n} frames");
    sizes.dedup();
    assert_eq!(sizes, [(1600, 1000), (1728, 1080), (640, 400)]);
}

#[test]
fn a_size_the_stream_didn_t_take_is_tried_again_until_it_is() {
    // Big, then back to Small, but the stream doesn't answer the first two tries: it goes back to
    // the small picture all the same, never staying big while the view is small.
    let backend = FakeBackend::new(Script {
        live_view_refuses: 2,
        ..Script::default()
    });
    let out = SharedOut::default();
    let screen = Target::Display {
        id: "display-1".into(),
    };
    let big = Some(ViewPixels {
        width: 1600,
        height: 1000,
    });
    let input = sent_at(&[
        (300, Command::ViewSize { view: None }),
        (2_300, Command::Stop),
    ]);
    let code = live_view_with(
        Box::new(backend),
        &screen,
        big,
        input,
        Box::new(out.clone()),
    );
    assert_eq!(code, 0);
    let mut sizes = sizes(&out.events());
    assert_eq!(sizes.last().copied(), Some((640, 400)), "small again");
    sizes.dedup();
    assert_eq!(sizes, [(1600, 1000), (640, 400)]);
}
