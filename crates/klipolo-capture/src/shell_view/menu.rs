//! The item's menu: the shell's own lines, the session's, then the actions the take's state allows, in the person's
//! words. Each action item carries its action, and a shell's native item carries the action's id (`MenuAction::id`),
//! so a click is paired with its action by id, never by its place in the menu.

use super::Words;
use crate::shell::{KeyBindings, MenuAction, StatusView, WorkerState};

/// One line of the item's menu: plain text (`action` None), or one of the take's actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub action: Option<MenuAction>,
    pub text: String,
}

/// The menu, top to bottom: `shell_lines` (the shell's own: `ready_line`, a taken shortcut), the session's lines (one
/// per source, the space left, already in the person's words), then the actions `menu_actions(view.state)` allows,
/// each worded as itself.
pub fn menu_items(view: &StatusView, words: &Words, shell_lines: &[String]) -> Vec<MenuItem> {
    let actions = menu_actions(view.state);
    let mut items = Vec::with_capacity(shell_lines.len() + view.lines.len() + actions.len());
    items.extend(
        (shell_lines.iter().chain(&view.lines)).map(|text| MenuItem {
            action: None,
            text: text.clone(),
        }),
    );
    items.extend(actions.iter().map(|&action| MenuItem {
        action: Some(action),
        text: action_text(action, view.state, words),
    }));
    items
}

/// `menu_items`' text alone, with no shell lines.
pub fn menu_lines(view: &StatusView, words: &Words) -> Vec<String> {
    (menu_items(view, words, &[]).into_iter())
        .map(|item| item.text)
        .collect()
}

/// The menu's actions for `state`: Pause or Resume, "Mark this moment" and "Stop and save" while the take runs;
/// "Cancel" (a Stop, which leaves no take) while armed or starting; none otherwise (Stopping and saving included).
/// Klipolo offers no Start over: a take that ends is saved and lands in the project.
pub fn menu_actions(state: WorkerState) -> &'static [MenuAction] {
    use MenuAction::{Marker, Pause, Resume, Stop};
    match state {
        WorkerState::Recording => &[Pause, Marker, Stop],
        WorkerState::Paused => &[Resume, Marker, Stop],
        WorkerState::Armed | WorkerState::Starting | WorkerState::Countdown => &[MenuAction::Stop],
        _ => &[],
    }
}

/// The armed item's first line, "Ready: press ⌥⌘R in your game", with the start key as this OS shows shortcuts
/// (`shortcut_text`). None in any other state, or when the start key isn't the person's to press (another app holds
/// it: the shell shows `KeysController::problem` instead).
pub fn ready_line(state: WorkerState, words: &Words, start_key: Option<&str>) -> Option<String> {
    let keys = start_key.filter(|_| state == WorkerState::Armed)?;
    Some(words.say("capture.menu.ready", &[("keys", keys.to_owned())]))
}

/// The app's Dock icon menu's actions for `state`: the item's own while the take records or is paused
/// (Pause or Resume, "Mark this moment", "Stop and save"), so the person can stop when the menu-bar item is hidden behind
/// the notch; none otherwise.
pub fn dock_actions(state: WorkerState) -> &'static [MenuAction] {
    use MenuAction::{Marker, Pause, Resume, Stop};
    match state {
        WorkerState::Recording => &[Pause, Marker, Stop],
        WorkerState::Paused => &[Resume, Marker, Stop],
        _ => &[],
    }
}

/// `action`'s words in `state`, as the item's menu says them ("Stop and save", or "Cancel" before the gate).
pub fn action_text(action: MenuAction, state: WorkerState, words: &Words) -> String {
    words.say(action_code(action, state), &[])
}

/// The shortcut a native menu shows beside `action` (in `KeyBindings`' syntax): Start over's, when the person set one.
/// The others' keys mean other things before the take starts (the start key starts it), so they show none.
pub fn menu_shortcut(action: MenuAction, keys: &KeyBindings) -> Option<&str> {
    let key = keys.start_over.as_deref().filter(|k| !k.is_empty());
    key.filter(|_| action == MenuAction::StartOver)
}

fn action_code(action: MenuAction, state: WorkerState) -> &'static str {
    match (action, state) {
        (MenuAction::Stop, WorkerState::Recording | WorkerState::Paused) => "capture.menu.stop",
        // Before the gate, Stop leaves no take: the person cancels.
        (MenuAction::Stop, _) => "capture.menu.cancel",
        (MenuAction::Pause, _) => "capture.menu.pause",
        (MenuAction::Resume, _) => "capture.menu.resume",
        (MenuAction::Marker, _) => "capture.menu.marker",
        (MenuAction::StartOver, _) => "capture.menu.startOver",
    }
}
