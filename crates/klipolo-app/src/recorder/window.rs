//! Klipolo's window during a take: hidden once the take records (so it is never in the person's way, nor in the
//! picture), shown and focused again when the take is saved, fails or is over (`events::carry_out`).

use tauri::{AppHandle, Manager};

/// The window the recorder hides and shows: Klipolo's main window, or a test's record.
pub trait Window: Send + Sync {
    fn hide(&self);
    fn show_and_focus(&self);
}

/// Klipolo's main window (`tauri.conf.json`'s `main`).
pub struct MainWindow(pub AppHandle);

impl Window for MainWindow {
    fn hide(&self) {
        if let Some(window) = self.0.get_webview_window("main")
            && let Err(error) = window.hide()
        {
            tracing::warn!(%error, "recorder: Klipolo's window didn't hide");
        }
    }

    fn show_and_focus(&self) {
        let Some(window) = self.0.get_webview_window("main") else {
            return;
        };
        let shown = (window.unminimize())
            .and_then(|()| window.show())
            .and_then(|()| window.set_focus());
        if let Err(error) = shown {
            tracing::warn!(%error, "recorder: Klipolo's window didn't come back");
        }
    }
}
