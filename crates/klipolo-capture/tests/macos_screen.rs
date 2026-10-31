//! ScreenCaptureKit's pure helpers on this Mac: the stream's frame interval and queue depth, the stop codes'
//! heal events and game sound's planar-to-interleaved step (the cursor's mapping is in `tests/cursor_map.rs`).
//! No stream is created: the real capture is the explicit smoke run's (`examples/screen_smoke/`), never a test's.
#![cfg(all(target_os = "macos", feature = "real-capture"))]

use klipolo_capture::pipe::PipeEvent;
use klipolo_capture::platform::macos::{frame_interval, interleave, pipe_event_for, queue_depth};

#[test]
fn the_frame_interval_is_ten_percent_shorter_than_the_target() {
    let t = frame_interval(60);
    assert!((t.value as f64 / t.scale as f64 - 1.0 / 66.0).abs() < 1e-6);
    assert_eq!((queue_depth(60), queue_depth(120)), (6, 8));
}

#[test]
fn screen_capture_kit_errors_map_to_the_heal_table() {
    assert_eq!(pipe_event_for(-3821), PipeEvent::StoppedBySystem);
    assert_eq!(pipe_event_for(-3817), PipeEvent::StoppedByUser);
    assert_eq!(pipe_event_for(-3801), PipeEvent::PermissionRevoked);
    assert_eq!(pipe_event_for(-3805), PipeEvent::Stalled);
}

#[test]
fn planar_game_sound_becomes_interleaved_stereo() {
    assert_eq!(
        interleave(&[1.0, 2.0], &[-1.0, -2.0]),
        [1.0, -1.0, 2.0, -2.0]
    );
}
