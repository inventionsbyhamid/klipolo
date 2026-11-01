//! The Dock icon's menu while a take records (`klipolo_platform::dock`): "Pause" or "Resume", "Mark this moment" and
//! "Stop and save", the menu-bar item's own actions in its words (`shell_view::dock_actions`), so the person can mark a
//! moment or stop when the item is hidden behind the notch. A click sends the same command the item's would, to the
//! take that runs; nothing here starts anything.

use klipolo_capture::protocol::Command;
use klipolo_capture::shell::MenuAction;
use klipolo_capture::shell_view::{Words, action_text, dock_actions};
use klipolo_platform::dock::DockItem;

use super::take::Recorder;

impl Recorder {
    /// The Dock menu's items now: none without a take that records or is paused.
    pub fn dock_items(&self) -> Vec<DockItem> {
        let slot = self.lock();
        let Some(take) = slot.take.as_ref().filter(|t| t.worker.is_some()) else {
            return Vec::new();
        };
        let Some(state) = take.sup.state() else {
            return Vec::new();
        };
        let words = Words::from(take.cfg.words.clone());
        (dock_actions(state).iter())
            .map(|&action| DockItem {
                id: action.id().to_owned(),
                title: action_text(action, state, &words),
            })
            .collect()
    }

    /// A click on the Dock menu's item `id`: its command to the take that runs (Stop is the person's, so the take is
    /// never resumed after it).
    pub fn dock_click(&self, id: &str) {
        let command = match MenuAction::from_id(id) {
            Some(MenuAction::Stop) => Command::Stop,
            Some(MenuAction::Pause) => Command::Pause,
            Some(MenuAction::Resume) => Command::Resume,
            Some(MenuAction::Marker) => Command::Marker,
            Some(MenuAction::StartOver) | None => {
                tracing::warn!(id, "recorder: a Dock item that isn't the recorder's");
                return;
            }
        };
        tracing::info!(?command, "recorder: the Dock menu's");
        self.send_current(command);
    }
}

/// Gives the app's Dock icon the recorder's menu (`klipolo_platform::dock::install`; on the main thread, from setup).
/// In a test run there is no recorder, so the menu holds the OS's own items only.
pub(crate) fn install(app: &tauri::AppHandle) {
    use tauri::Manager;
    let (items, clicks) = (app.clone(), app.clone());
    klipolo_platform::dock::install(
        Box::new(move || {
            (items.try_state::<super::RecorderState>())
                .map(|state| state.dock_items())
                .unwrap_or_default()
        }),
        Box::new(move |id| {
            if let Some(state) = clicks.try_state::<super::RecorderState>() {
                state.dock_click(id);
            }
        }),
    );
}
