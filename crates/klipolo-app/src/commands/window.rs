//! The window's full screen, for the full screen preview (F): the page puts the window in and out of it, and asks again
//! once a resize settles, as the window can also leave it another way (the green button, ⌃⌘F).

use tauri::WebviewWindow;

/// Puts the window in full screen (`on`) or takes it out; says whether it was in full screen before, so the page takes
/// out only a window it put in. Sync: it runs on the main thread, where the window lives.
#[tauri::command]
pub(crate) fn window_full_screen(window: WebviewWindow, on: bool) -> Result<bool, String> {
    let was = window.is_fullscreen().map_err(|e| e.to_string())?;
    if was != on {
        window.set_fullscreen(on).map_err(|e| e.to_string())?;
    }
    Ok(was)
}

/// Whether the window is in full screen now.
#[tauri::command]
pub(crate) fn window_is_full_screen(window: WebviewWindow) -> Result<bool, String> {
    window.is_fullscreen().map_err(|e| e.to_string())
}
