//! Capture begins only when the person presses Record or Check camera: never in tests, builds or device inspection.
//! The exceptions, the recorder's live view and the Record dialog's stills, are capture files of their own
//! (`captures`). These checks read source text only, every OS's on every OS, so a Linux run checks the macOS and
//! Windows code too, and each later task's files are checked as they land.

// Only the source helpers, not `common`: a source scan links nothing else (no FFmpeg).
#[path = "common/scan.rs"]
mod scan;
// The cursor's rules, beside these.
#[path = "consent_scan/cursor.rs"]
mod cursor;

use std::path::Path;

use scan::{code_of, first_statement, names_word, read, rs_files, sources};

/// Device inspection never activates capture: listing files never call cpal's probing
/// iterators, and the preflight files never touch a capture API.
#[test]
fn listing_code_never_opens_a_device() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let cpal_listing = [
        "input_devices(",
        "supported_input_configs(",
        "default_input_config(",
    ];
    let mut checked = 0;
    for os in ["macos", "windows", "linux"] {
        for file in ["devices.rs", "displays.rs", "permissions.rs", "apps.rs"] {
            let path = root.join("platform").join(os).join(file);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            checked += 1;
            for banned in cpal_listing.iter().chain(&[
                "ShareableContent",
                "sc::Stream",
                "CaptureSession",
                "startRunning",
                "build_input_stream",
                "AccessCamera",
                "Screencast",
            ]) {
                assert!(!text.contains(banned), "{} names {banned}", path.display());
            }
        }
    }
    // The cpal microphone finds its device by name without the probing iterator; its config is read only in start().
    let mic = std::fs::read_to_string(root.join("mic.rs")).unwrap();
    assert!(!mic.contains("input_devices(") && !mic.contains("supported_input_configs("));
    println!("{checked} listing files checked");
}

/// Calls that start capture, open a capture device or ask the person for permission, by the names each OS's crates
/// give them. Only a capture file (`captures`) may name one, and it runs only after Record or Check camera.
const CAPTURE_APIS: &[&str] = &[
    // macOS: ScreenCaptureKit's content list (the monthly prompt), stream and screenshots, AVFoundation's session and
    // device inputs, the permission requests, Core Audio's I/O.
    "ShareableContent",
    "sc::Stream",
    "ScreenshotManager",
    "CaptureSession",
    "startRunning",
    "CaptureDeviceInput",
    "requestAccess",
    "request_access",
    "CGRequestScreenCaptureAccess",
    "request_screen_capture_access",
    "AudioOutputUnitStart",
    "AudioDeviceStart",
    // cpal: an input stream, and the config read that opens the device (an AUHAL unit, a WASAPI `IAudioClient`).
    "build_input_stream",
    "default_input_config(",
    // Windows: Graphics Capture, desktop duplication, WASAPI's clients and process loopback, Media Foundation's camera.
    "GraphicsCaptureItem",
    "CreateForMonitor",
    "CreateForWindow",
    "DuplicateOutput",
    "IAudioClient",
    "IAudioCaptureClient",
    "ActivateAudioInterfaceAsync",
    "MFCreateDeviceSource",
    "ActivateObject",
    "MFCreateSourceReader",
    // Linux: the ScreenCast and Camera portals, PipeWire streams, V4L2 streaming, X11 image grabs.
    "Screencast",
    "AccessCamera",
    "open_pipe_wire_remote",
    "pw::stream",
    "pipewire::stream",
    "MmapStream",
    "UserptrStream",
    "get_image",
    // The mouse and the cursor: macOS's global (and local) monitors and cursor image, Raw Input, XInput2 and
    // XFixes.
    "addGlobalMonitorForEvents",
    "addLocalMonitorForEvents",
    "currentSystemCursor",
    "currentSystem",
    "RegisterRawInputDevices",
    "XISelectEvents",
    "SelectCursorInput",
];

/// cpal's listing, which opens every device to filter it: microphones are never listed through cpal,
/// not even by a capture file.
const CPAL_PROBES: &[&str] = &["input_devices(", "supported_input_configs("];

/// The files that capture, once the person pressed Record or Check camera (the plan's file map): each OS's pipes, Cap's
/// vendored capture code and the cpal microphone. Everything else (devices, permissions, apps, preflight, the probe,
/// shells, the session) must not name a capture API.
fn captures(file: &str) -> bool {
    const PIPES: &[&str] = &[
        "platform/macos/screen.rs",
        // The screen pipe's stream settings, applied live by the cursor's fallback.
        "platform/macos/screen/setup.rs",
        "platform/macos/sound.rs",
        "platform/macos/filter.rs",
        "platform/macos/camera.rs",
        "platform/macos/preview.rs",
        // Screen Recording's own request (`CGRequestScreenCaptureAccess`): it runs only on the person's press of "Allow
        // screen recording", through `Backend::request_screen_permission` (the app's caller is
        // pinned below).
        "platform/macos/screen_permission.rs",
        // The recorder's live view of the chosen screen or window: the one exception to capture only after a press.
        // It runs only while the recorder is open and in front and screen recording is already allowed
        // (its `start` reads the preflight before the content list), and never opens the camera or the mic.
        "platform/macos/live_view.rs",
        // The Record dialog's stills of each screen and app, under the live view's rule: only while the dialog is
        // open and in front and screen recording is already allowed (each still reads the preflight before the content
        // list), never the camera or the mic.
        "platform/macos/thumbs.rs",
        "platform/windows/wgc.rs",
        "platform/windows/dda.rs",
        "platform/windows/screen.rs",
        "platform/windows/loopback.rs",
        "platform/windows/camera.rs",
        "platform/windows/preview.rs",
        "platform/linux/portal.rs",
        "platform/linux/pipewire_video.rs",
        "platform/linux/x11.rs",
        "platform/linux/screen.rs",
        "platform/linux/pipewire_audio.rs",
        "platform/linux/pipewire_mic.rs",
        "platform/linux/camera.rs",
        "platform/linux/camera_portal.rs",
        "platform/linux/preview.rs",
        "mic/cpal_mic.rs",
    ];
    let cursor_source = ["macos", "windows", "linux"]
        .iter()
        .any(|os| file.starts_with(&format!("platform/{os}/cursor")));
    PIPES.contains(&file) || file.starts_with("vendor/") || cursor_source
}

#[test]
fn only_the_capture_files_name_a_capture_api_and_none_lists_through_cpal() {
    let mut checked = 0;
    for (file, text) in sources() {
        let code = code_of(&text);
        for probe in CPAL_PROBES {
            assert!(
                !code.contains(probe),
                "src/{file} lists through cpal ({probe})"
            );
        }
        if captures(&file) {
            continue;
        }
        checked += 1;
        for api in CAPTURE_APIS {
            assert!(
                !code.contains(api),
                "src/{file} names {api}: only a capture file may, after Record"
            );
        }
    }
    // The mic's own files, comments included, never even mention the probing calls.
    for (file, text) in sources().filter(|(f, _)| f == "mic.rs" || f.starts_with("mic/")) {
        for probe in CPAL_PROBES {
            assert!(!text.contains(probe), "src/{file} names {probe}");
        }
    }
    assert!(checked >= 40, "only {checked} files checked");
}

/// ScreenCaptureKit's screenshot is the Record dialog's stills' alone: no other file, capture file or not, names it,
/// so no screenshot is taken anywhere without the stills' preflight-first rule (`tests/thumbs.rs` checks its order).
#[test]
fn only_the_stills_take_a_screenshot() {
    let named: Vec<String> = sources()
        .filter(|(_, text)| code_of(text).contains("ScreenshotManager"))
        .map(|(file, _)| file)
        .collect();
    assert_eq!(named, ["platform/macos/thumbs.rs"]);
}

/// Screen Recording's request runs only on the person's press of "Allow screen recording": nothing in this
/// crate calls `Backend::request_screen_permission`, and in every other crate only the app recorder's permission
/// commands do (none before the app has a recorder).
#[test]
fn only_the_allow_screen_press_requests_screen_recording() {
    let calls = |text: &str| code_of(text).contains(".request_screen_permission(");
    let here: Vec<String> = sources()
        .filter(|(_, t)| calls(t))
        .map(|(f, _)| f)
        .collect();
    assert!(here.is_empty(), "called in src/: {here:?}");
    // Every other crate's `src/`, the app's included, as the begin_capture scan reads them.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let (mut read, mut callers) = (0, vec![]);
    for entry in std::fs::read_dir(&crates).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name == "klipolo-capture" || !path.join("src").is_dir() {
            continue;
        }
        for (file, text) in rs_files(&path.join("src")) {
            read += 1;
            if calls(&text) {
                callers.push(format!("{name}/src/{file}"));
            }
        }
    }
    callers.sort();
    let expected: &[&str] = if crates.join("klipolo-app/src/recorder").is_dir() {
        &["klipolo-app/src/recorder/permissions.rs"]
    } else {
        &[]
    };
    assert_eq!(callers, expected);
    assert!(read >= 100, "only {read} files of the other crates read");
}

/// Every real pipe asks the consent guard, which panics under `KLIPOLO_TEST` (the test commands and CI all
/// set it). The cpal microphone asks first thing in `new` and in `start`, and `new` opens nothing.
#[test]
fn every_real_pipe_asks_the_guard_and_the_mic_opens_nothing_before_start() {
    for (file, text) in sources().filter(|(f, _)| !f.starts_with("synthetic/")) {
        let code = code_of(&text);
        let pipe = [
            "AudioPipe for",
            "VideoPipe for",
            "PreviewPipe for",
            "CursorSource for",
        ]
        .iter()
        .any(|i| code.contains(i));
        assert!(
            !pipe || code.contains("assert_real_capture_allowed("),
            "src/{file}: a pipe without the consent guard"
        );
    }
    let mic = code_of(&read("mic/cpal_mic.rs"));
    for f in ["pub fn new(", "fn start("] {
        assert!(
            first_statement(&mic, f).contains("assert_real_capture_allowed("),
            "CpalMic's {f}…) must ask the guard first"
        );
    }
    // `new`'s body: from its signature to the next function.
    let new = &mic[mic.find("pub fn new(").unwrap() + "pub fn new(".len()..];
    let new = &new[..new.find("fn ").unwrap_or(new.len())];
    for touch in [
        "cpal::",
        "devices(",
        "default_input_config(",
        "build_input_stream",
        ".play(",
        "spawn",
    ] {
        assert!(
            !new.contains(touch),
            "CpalMic::new touches the device ({touch}); only start may"
        );
    }
}

/// A real pipe's start can wait on the system (macOS's screen-recording or camera prompt, a picker): each one checks
/// the person's Stop (`StopFlag`) right before it opens its device, so nothing opens after it. Check
/// camera's preview pipes too (its start can wait on the camera's prompt).
#[test]
fn every_real_pipe_checks_the_person_s_stop_before_it_opens() {
    let mut pipes = 0;
    for (file, text) in sources().filter(|(f, _)| !f.starts_with("synthetic/")) {
        let code = code_of(&text);
        if ["AudioPipe for", "VideoPipe for", "PreviewPipe for"]
            .iter()
            .any(|i| code.contains(i))
        {
            pipes += 1;
            assert!(
                code.contains("stopping.is_set()"),
                "src/{file}: a pipe that opens without checking the person's Stop"
            );
        }
    }
    assert!(pipes >= 3, "only {pipes} real pipes found");
}

/// The take's power activity (`klipolo_platform::power::begin_capture`) is begun only by the shells' shared view, which
/// holds it only while a take the person started runs (`shell_view::TakeActivity`): no listing, preflight, probe or
/// session code takes it, and it opens no device. Any mention in code counts, not only a call, so an alias
/// (`use … as hold`) or the function passed around as a value can't get past; and every other crate and the app are
/// read too, where only klipolo-platform, which defines it, may name it.
#[test]
fn only_the_shell_view_begins_the_take_s_activity() {
    let names = |text: &str| names_word(&code_of(text), "begin_capture");
    let named: Vec<String> = sources()
        .filter(|(_, text)| names(text))
        .map(|(file, _)| file)
        .collect();
    assert_eq!(named, ["shell_view.rs"]);
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // The app is one of the crates.
    let mut dirs = vec![];
    for entry in std::fs::read_dir(workspace.join("crates")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !["klipolo-capture", "klipolo-platform"].contains(&name.as_str()) {
            dirs.push(path.join("src"));
        }
    }
    let (mut read, mut named) = (0, vec![]);
    for dir in dirs.iter().filter(|d| d.is_dir()) {
        for (file, text) in rs_files(dir) {
            read += 1;
            if names(&text) {
                named.push(dir.join(file));
            }
        }
    }
    assert!(named.is_empty(), "outside the shell view: {named:?}");
    assert!(read >= 100, "only {read} files of the other crates read");
}

/// cpal is built only for the real backends, and never for Linux: a test or `synthetic` build has no capture code to
/// call, and Linux records the mic through PipeWire (cpal's ALSA stream isn't on the host clock).
#[test]
fn cpal_is_built_only_with_real_capture_and_never_for_linux() {
    let toml =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let (mut section, mut deps, mut features) = ("", vec![], vec![]);
    for line in toml.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        if line.starts_with('[') {
            section = line;
        } else if line.starts_with("cpal") {
            deps.push((section, line));
        } else if section == "[features]" && line.contains("cpal") {
            features.push(line);
        }
    }
    assert_eq!(deps.len(), 1, "one cpal dependency: {deps:?}");
    assert_eq!(
        deps[0].0,
        r#"[target.'cfg(any(target_os = "macos", windows))'.dependencies]"#
    );
    assert!(deps[0].1.contains("optional = true"), "{}", deps[0].1);
    assert!(
        features.len() == 1
            && features[0].starts_with("real-capture = ")
            && features[0].contains(r#""dep:cpal""#),
        "only real-capture turns cpal on: {features:?}"
    );
}
