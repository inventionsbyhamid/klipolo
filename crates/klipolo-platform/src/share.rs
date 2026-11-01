//! The system's share sheet, for files Klipolo made (the finished export's Share…). macOS: `NSSharingServicePicker`,
//! the menu of the Mac's sharing services (AirDrop, Messages, Mail and the person's other apps) for those files, shown
//! beside a place in the window. Windows and Linux have none here: `supported` is false and nothing shows.

use std::ffi::c_void;
use std::path::PathBuf;

/// Where the share menu shows: a rect in the view's own coordinates (a webview's are flipped, with the top-left origin
/// the page has). The menu opens below it, or wherever the screen leaves room.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// This computer has a share sheet.
pub fn supported() -> bool {
    crate::imp::share_supported()
}

/// Shows the share menu for `files` beside `anchor` in `view` (macOS: a live NSView of the window, such as Tauri's
/// WKWebView; call it on the main thread). True once it showed; false off the main thread, with no view or no file it
/// can name, or on a computer without a share sheet. The person picks a service, or dismisses the menu, in AppKit.
pub fn show(view: *mut c_void, anchor: Anchor, files: &[PathBuf]) -> bool {
    crate::imp::share_files(view, anchor, files)
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn without_a_share_sheet_nothing_shows() {
        assert!(!supported());
        let files = [PathBuf::from("/tmp/Trip 16x9.mp4")];
        assert!(!show(std::ptr::null_mut(), Anchor::default(), &files));
    }
}
