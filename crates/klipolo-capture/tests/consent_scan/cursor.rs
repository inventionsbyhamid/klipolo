//! The cursor's consent and privacy rules: mouse only, never a key, a window
//! title or a prompt, anywhere in this crate (vendored files included, and an alias or a re-export can't hide one: it
//! names the item it aliases), and every real cursor source asks the consent guard first.

use super::scan::{code_of, names_word, sources};

/// Names that read keys, key codes or modifier state, or ask the person for a permission: never in this crate. Matched
/// as whole words (an import alias names its original too).
const KEYBOARD_AND_PROMPTS: &[&str] = &[
    // macOS: key event masks, taps, key state, the listen-event and Accessibility prompts.
    "NSEventMaskKeyDown",
    "NSEventMaskKeyUp",
    "NSEventMaskFlagsChanged",
    "kCGEventKeyDown",
    "kCGEventKeyUp",
    "kCGEventFlagsChanged",
    "CGEventTapCreate",
    "CGEventSourceKeyState",
    "CGRequestListenEventAccess",
    "AXIsProcessTrustedWithOptions",
    // Windows: hooks and key state.
    "SetWindowsHookEx",
    "SetWindowsHookExA",
    "SetWindowsHookExW",
    "WH_KEYBOARD",
    "WH_KEYBOARD_LL",
    "GetKeyState",
    "GetKeyboardState",
    "HID_USAGE_GENERIC_KEYBOARD",
    // X11 and Linux: key events and raw input devices.
    "XI_KeyPress",
    "XI_RawKeyPress",
    "XI_KeyRelease",
    "XI_RawKeyRelease",
    "KeyPressMask",
    "device_query",
    "rdev",
];

/// Text a keyboard read takes that isn't a single name.
const KEYBOARD_TEXT: &[&str] = &[
    "NSEventMask::KeyDown",
    "NSEventMask::KeyUp",
    "NSEventMask::FlagsChanged",
    "usUsage: 6",
    "usUsage: 0x06",
    "usUsage: 0x6",
    "/dev/input",
    "KeyboardAndMouse::*",
];

/// The mouse buttons `GetAsyncKeyState` may be asked about (a sampled fallback's button check); never a key.
const MOUSE_BUTTONS: &[&str] = &[
    "VK_LBUTTON",
    "VK_RBUTTON",
    "VK_MBUTTON",
    "VK_XBUTTON1",
    "VK_XBUTTON2",
];

/// Window titles: never read by a cursor source (bounds only).
const TITLES: &[&str] = &[
    "kCGWindowName",
    "kCGWindowOwnerName",
    "GetWindowText",
    "_NET_WM_NAME",
    "WM_NAME",
];

/// A file of the cursor: the seam and its writer, each OS's source, the shared helpers, the synthetic source.
pub fn cursor_file(file: &str) -> bool {
    file == "cursor.rs"
        || file.starts_with("cursor/")
        || file == "synthetic/cursor.rs"
        || file.starts_with("platform/common/cursor_")
        || ["macos", "windows", "linux"].iter().any(|os| {
            file.starts_with(&format!("platform/{os}/cursor.rs"))
                || file.starts_with(&format!("platform/{os}/cursor/"))
        })
}

#[test]
fn no_keyboard_title_or_prompting_call_in_capture() {
    let (mut checked, mut mac_cursor) = (0, 0);
    for (file, text) in sources() {
        checked += 1;
        mac_cursor += usize::from(file.starts_with("platform/macos/cursor/"));
        let code = code_of(&text);
        for name in KEYBOARD_AND_PROMPTS {
            assert!(!names_word(&code, name), "src/{file} names {name}");
        }
        for text in KEYBOARD_TEXT {
            assert!(!code.contains(text), "src/{file} has {text}");
        }
        for (at, _) in code.match_indices("GetAsyncKeyState(") {
            let arg = code[at + "GetAsyncKeyState(".len()..]
                .split(')')
                .next()
                .unwrap_or("");
            let arg = arg.trim().rsplit("::").next().unwrap_or("");
            assert!(
                MOUSE_BUTTONS.contains(&arg),
                "src/{file} asks GetAsyncKeyState about {arg}"
            );
        }
        if cursor_file(&file) {
            for title in TITLES {
                assert!(
                    !code.contains(title),
                    "src/{file} reads a window's title ({title})"
                );
            }
        }
    }
    assert!(checked >= 150, "only {checked} files checked");
    // The macOS source's parts (the monitor, the sampler, the shapes, the bounds, the poll) are read too.
    assert!(mac_cursor >= 5, "only {mac_cursor} macOS cursor files read");
    // The scan itself catches what it bans (a plain name, an alias, a masked key bit).
    let planted = "use objc2_app_kit::NSEventMaskKeyDown as Quiet;\nlet m = NSEventMask::KeyDown;";
    assert!(names_word(planted, "NSEventMaskKeyDown") && planted.contains("NSEventMask::KeyDown"));
    assert!(!names_word("let pressedRdevice = 1;", "rdev"));
}

/// The real cursor sources: each `new` asks the guard first, as the pipes do. macOS's is pinned.
#[test]
fn every_cursor_source_asks_the_guard_first() {
    let (mut found, mut mac) = (0, false);
    for (file, text) in sources().filter(|(f, _)| !f.starts_with("synthetic/")) {
        let code = code_of(&text);
        for ty in ["MacCursorSource", "WinCursorSource", "LinuxCursorSource"] {
            let Some(at) = code.find(&format!("impl {ty} {{")) else {
                continue;
            };
            found += 1;
            mac |= ty == "MacCursorSource";
            let body = &code[at..];
            let new = body
                .find("fn new(")
                .unwrap_or_else(|| panic!("src/{file}: {ty} has no new"));
            assert!(
                super::scan::first_statement(&body[new..], "fn new(")
                    .contains("assert_real_capture_allowed(\"cursor\")"),
                "src/{file}: {ty}::new must ask the guard first"
            );
        }
        if code.contains("CursorSource for") {
            assert!(
                code.contains("assert_real_capture_allowed("),
                "src/{file}: a cursor source without the guard"
            );
        }
    }
    assert!(mac, "MacCursorSource::new isn't where the scan reads it");
    println!("{found} real cursor sources checked");
}
