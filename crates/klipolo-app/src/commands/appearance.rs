//! Settings' Appearance on the window: its own colour, behind the see-through page, is the canvas of the theme in use.
//! Painted at launch before the page loads (a light launch never shows the dark colour first), when the person picks
//! another theme, and when the system's appearance changes while they follow it. Thin over `klipolo_app::appearance`
//! and `prefs`, where the pick is saved.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use klipolo_app::appearance;
use klipolo_app::prefs::{self, Theme};
use tauri::window::Color;
use tauri::{AppHandle, Manager, State, Window};

/// The theme in use, and the support folder it is saved in (none without a home folder: it lasts this run).
pub(crate) struct AppearanceState {
    dir: Option<PathBuf>,
    theme: Mutex<Theme>,
}

impl AppearanceState {
    fn theme(&self) -> Theme {
        *self.theme.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Paints `window` with the canvas of `theme`, the system's appearance being `system`.
fn paint(window: &Window, theme: Theme, system: tauri::Theme) {
    let [r, g, b] = appearance::canvas(theme, system == tauri::Theme::Dark);
    if let Err(error) = window.set_background_color(Some(Color(r, g, b, 255))) {
        tracing::warn!(%error, "the window's colour couldn't follow the theme");
    }
}

/// The main window, as a window (its colour, not its webview's: the page stays see-through over the preview).
fn main_window(app: &AppHandle) -> Option<Window> {
    app.get_webview_window("main")
        .map(|window| window.as_ref().window())
}

/// At launch, from the setup hook: the window exists and hasn't drawn yet. Reads the saved theme and paints.
pub(crate) fn at_launch(app: &AppHandle) {
    let dir = klipolo_mcp::connection::support_dir();
    let theme = dir
        .as_deref()
        .map(|dir| prefs::load(dir).theme)
        .unwrap_or_default();
    if let Some(window) = main_window(app) {
        let system = window.theme().unwrap_or(tauri::Theme::Dark);
        paint(&window, theme, system);
    }
    app.manage(AppearanceState {
        dir,
        theme: Mutex::new(theme),
    });
}

/// The system's appearance changed (`WindowEvent::ThemeChanged`): the window follows it while the theme is System.
pub(crate) fn system_changed(app: &AppHandle, system: tauri::Theme) {
    let Some(state) = app.try_state::<AppearanceState>() else {
        return;
    };
    if state.theme() == Theme::System
        && let Some(window) = main_window(app)
    {
        paint(&window, Theme::System, system);
    }
}

/// Settings' Appearance: the theme picked, saved for the next launch and painted now. The page sends it as it loads
/// too, so a pick kept only by the page reaches the window.
#[tauri::command]
pub(crate) fn appearance_set(window: Window, state: State<'_, AppearanceState>, theme: Theme) {
    let before = std::mem::replace(
        &mut *state.theme.lock().unwrap_or_else(PoisonError::into_inner),
        theme,
    );
    // Saved only when it changed: the page sends the theme it has each time it loads.
    if before != theme
        && let Some(dir) = &state.dir
        && let Err(error) = prefs::update(dir, |prefs| prefs.theme = theme)
    {
        tracing::warn!(%error, "the theme couldn't be saved");
    }
    let system = window.theme().unwrap_or(tauri::Theme::Dark);
    paint(&window, theme, system);
}
