//! The camera preview's tests: a close is enforced, and a closing preview's frames never reach the page; its frames
//! carry the camera's id. Fake workers only: nothing captures.

use klipolo_capture::protocol::Command;

use super::test_previews::*;
use super::types::{PreviewKind, Previews};

const CAMERA: &str = "--check-camera";

/// The camera's preview alone.
fn camera() -> Previews {
    Previews {
        camera: Some("camera-1".into()),
        ..nothing()
    }
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

/// A preview that doesn't stop when it is closed is killed 2 s later.
#[test]
fn a_preview_that_doesn_t_stop_is_killed() {
    let p = previewing(all_allowed());
    p.set(camera()).unwrap();
    p.set(nothing()).unwrap();
    assert_eq!(p.launch.commands(), [Command::Stop]);
    p.advance_ms(1_900);
    assert!(p.launch.kills().is_empty(), "within its wait");
    p.advance_ms(1_100);
    assert_eq!(p.launch.kills(), [true], "hard-stopped after the wait");
}
