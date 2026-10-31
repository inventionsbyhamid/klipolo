//! The camera preview's tests: arming closes it before the take starts, a close is enforced, and a closing preview's
//! frames never reach the page; its frames carry the camera's id. Fake workers only: nothing captures.

use klipolo_capture::protocol::Command;

use super::test_previews::*;
use super::test_support::*;
use super::types::{PreviewKind, Previews};

const CAMERA: &str = "--check-camera";

/// The camera's preview alone.
fn camera() -> Previews {
    Previews {
        camera: Some("camera-1".into()),
        ..nothing()
    }
}

/// Record closes the camera's preview before the take starts: the preview is killed at arming, then the take's Start
/// goes out.
#[test]
fn record_closes_the_camera_preview_before_the_take_starts() {
    let p = previewing(all_allowed());
    p.set(camera()).unwrap();
    assert_eq!(
        p.launch.last_args(),
        ["--record-worker", CAMERA, "camera-1"]
    );
    p.arm(options_screen()).unwrap();
    p.send(Command::Start).unwrap();
    assert_eq!(p.launch.kills(), [true]);
    assert_eq!(p.launch.commands(), [Command::Start]);
}

/// Its frames reach the page under the camera's id; once it closes, a frame it still sends never does.
#[test]
fn its_frames_carry_the_camera_and_stop_when_it_closes() {
    let p = previewing(all_allowed());
    p.set(camera()).unwrap();
    let preview = p.worker(CAMERA);
    p.says(preview, frame());
    p.set(nothing()).unwrap();
    assert_eq!(p.told(preview), [Command::Stop]);
    p.says(preview, frame());
    let frames = p.frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].0.kind, PreviewKind::Camera);
    assert_eq!(frames[0].0.key, "camera-1");
}

/// A preview that doesn't stop when it is closed is killed 2 s later, and one still open when Klipolo exits is killed
/// then: it never outlives Klipolo with the camera on.
#[test]
fn a_preview_that_doesn_t_stop_is_killed_and_none_outlives_the_app() {
    let p = previewing(all_allowed());
    p.set(camera()).unwrap();
    p.set(nothing()).unwrap();
    assert_eq!(p.launch.commands(), [Command::Stop]);
    p.advance_ms(1_900);
    assert!(p.launch.kills().is_empty(), "within its wait");
    p.advance_ms(1_100);
    assert_eq!(p.launch.kills(), [true], "hard-stopped after the wait");
    p.set(camera()).unwrap();
    p.state.previews_exit();
    assert_eq!(p.launch.kills(), [true, true], "at Klipolo's exit");
}
