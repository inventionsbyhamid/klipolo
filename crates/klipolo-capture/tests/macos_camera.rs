//! The camera's format choice on the Mac: the smallest that meets the preset, else the largest below; a slow
//! camera still records. Pure; no camera is opened.
#![cfg(all(target_os = "macos", feature = "real-capture"))]
use klipolo_capture::platform::macos::{FormatFacts, pick_format};

fn f(w: u32, h: u32, fps: f64, nv12: bool) -> FormatFacts {
    FormatFacts {
        width: w,
        height: h,
        max_fps: fps,
        nv12,
    }
}

#[test]
fn the_camera_format_is_the_smallest_that_meets_the_preset() {
    let formats = [
        f(640, 480, 30.0, true),
        f(1280, 720, 30.0, true),
        f(1920, 1080, 30.0, true),
        f(1920, 1080, 60.0, false),
    ];
    assert_eq!(pick_format(&formats, (1280, 720, 30)), Some(1));
    assert_eq!(
        pick_format(&formats, (3840, 2160, 30)),
        Some(2),
        "the largest below when none meets it"
    );
    assert_eq!(
        pick_format(&[f(1280, 720, 15.0, true)], (1280, 720, 30)),
        Some(0),
        "a slow camera still records"
    );
}
