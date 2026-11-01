//! The recorder's live view never asks for a permission and never opens the camera or the mic (the one exception
//! to capture only after a press): the source scan for its two files, apart from `consent_scan.rs`. They are read as
//! text on every OS.

// Only the source helpers, not `common`: a source scan links nothing else (no FFmpeg).
#[path = "common/scan.rs"]
#[allow(dead_code, reason = "this scan needs two of the shared source helpers")]
mod scan;

use scan::{code_of, read};

/// The Mac's live view reads Screen Recording's preflight (which never prompts) before the content list (which can show
/// macOS's prompt), and its worker reads the permission before it builds the pipe. Neither requests a permission, opens
/// a camera or a mic, captures sound or writes a take.
#[test]
fn the_live_view_reads_the_permission_first_and_never_asks_or_opens_a_camera_or_a_mic() {
    let pipe = code_of(&read("platform/macos/live_view.rs"));
    let preflight = pipe.find("CGPreflightScreenCaptureAccess()");
    let content = pipe.find("filter::content()").expect("the content list");
    assert!(
        preflight.is_some_and(|p| p < content),
        "the preflight comes before the content list"
    );
    let worker = code_of(&read("run/live_view.rs"));
    let permission = worker.find(".permissions()");
    let built = worker.find(".live_view(").expect("the pipe built");
    assert!(
        permission.is_some_and(|p| p < built),
        "the worker reads the permission before it builds the pipe"
    );
    for (file, code) in [("live view", &pipe), ("its worker", &worker)] {
        for banned in [
            "CGRequestScreenCaptureAccess",
            "request_screen_permission",
            "request_access",
            "requestAccess",
            "CaptureSession",
            "CaptureDevice",
            "AudioSpec",
            "cpal",
            "set_captures_audio",
            "SessionDir",
        ] {
            assert!(!code.contains(banned), "the {file} names {banned}");
        }
    }
}
