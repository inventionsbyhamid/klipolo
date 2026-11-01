//! The camera opens nothing once the person stopped ("a camera light flash after Stop is not acceptable"): the
//! source scan for the camera's open sites, apart from `consent_scan.rs`. Every OS's camera files are read as text on
//! every OS.

// Only the source helpers, not `common`: a source scan links nothing else (no FFmpeg).
#[path = "common/scan.rs"]
#[allow(dead_code, reason = "this scan needs two of the shared source helpers")]
mod scan;

use scan::{code_of, read};

/// The camera's open sites ("a camera light flash after Stop is not acceptable"): in each camera file, the
/// prompt, the device input and `startRunning` each come right after a check of the person's Stop in the same function,
/// with no other open site between them, so the camera never asks or opens once the person stopped.
#[test]
fn every_camera_open_site_comes_right_after_a_check_of_the_person_s_stop() {
    const OPENS: &[&str] = &[
        "request_access",
        "DeviceInput::with_device(",
        "start_running(",
    ];
    const CAMERA_FILES: &[&str] = &["platform/macos/camera.rs", "platform/macos/preview.rs"];
    let mut sites = 0;
    for file in CAMERA_FILES {
        let code = code_of(&read(file));
        let mut at: Vec<(usize, &str)> = OPENS
            .iter()
            .flat_map(|open| code.match_indices(open).map(|(i, _)| (i, *open)))
            .collect();
        at.sort();
        for (k, &(i, open)) in at.iter().enumerate() {
            sites += 1;
            let before = &code[..i];
            let function = before.rfind("fn ").expect("an open site inside a function");
            let check = before.rfind("stopping.is_set()");
            let previous = k.checked_sub(1).map(|j| at[j].0);
            assert!(
                check.is_some_and(|c| c > function && previous.is_none_or(|p| c > p)),
                "src/{file}: {open} opens without a check of the person's Stop right before it"
            );
        }
    }
    assert!(sites >= 3, "only {sites} camera open sites found");
}
