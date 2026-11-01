//! The eyedropper: `klipolo_platform::screen_color` on the main thread.

use klipolo_platform::screen_color;
use tauri::WebviewWindow;

/// Shows the system's eyedropper and answers the colour the person clicked, as sRGB `#RRGGBB`; None when they
/// cancelled, one is already showing, or this computer has none. Only the person's press on the color popover's
/// eyedropper calls it: it is no tool, so chat and MCP apps can't start it.
#[tauri::command]
pub async fn screen_color_pick(window: WebviewWindow) -> Option<String> {
    if !screen_color::supported() {
        return None;
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let shown = window.run_on_main_thread(move || {
        screen_color::sample(Box::new(move |hex| {
            let _ = tx.send(hex);
        }));
    });
    if let Err(error) = shown {
        tracing::warn!(%error, "the eyedropper couldn't reach the main thread");
        return None;
    }
    // The main-thread task dropped unrun (the event loop gone), or the sender dropped: nothing picked. (Quitting while
    // the loupe shows never answers: the process ends with this task.)
    rx.await.unwrap_or(None)
}
