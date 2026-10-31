//! The platform-neutral half of every OS's recorder shell (the macOS, Windows and Linux shells build on it): the
//! 21, 26 and 30 build on it): the status item's title and its menu's lines, the person's words for native menus, the
//! hotkeys registered only while armed, and the take's power activity held only while a take the person started runs.
//!
//! - **The title** (`status_title`): "● 12:34" while recording, "❚❚ 12:34" while paused, `h:mm:ss` from an hour.
//! - **The menu** (`menu_items`): the shell's own lines (the armed "Ready: press ⌥⌘R in your game", `ready_line`, with
//!   the start key as the OS shows it, `shortcut_text`; a taken shortcut), one plain line per source and the space
//!   left, as the session words them (`StatusView::lines`), then the actions the state allows (`menu_actions`), in the
//!   person's words. Each action item carries its action, paired with the native item by id (`MenuAction::id`).
//! - **The HUD** (`HudView`): what it shows and when, for the shells that have one.
//! - **The camera bubble** (`bubble_shows`): when the take's live round picture of its camera shows, for the shells
//!   that draw one.
//! - **Words**: the session's `capture.*` templates in the person's language (`SessionConfig::words`), with this
//!   crate's English for any code it doesn't word (Rust sends codes; the shells word them as the UI does).
//! - **Keys** (`KeysController`): the session's bindings are registered only between `ShellOutput::Keys(true)` and
//!   `Keys(false)`, so a hotkey never starts anything by itself.
//! - **A system quit** (`quit_wait`): a take stops and saves within a bound before the process ends with the session's
//!   own exit code.
//! - **The take's activity** (`TakeActivity`): `klipolo_platform::power::begin_capture`, held from the gate until Stop's
//!   finishing ends and never while armed. It is that call's only caller (`tests/consent_scan.rs`), and it opens no
//!   device: only the person's Record (or their start key) brings a take to Recording.
//!
//! Costs: a title or a menu is a few short strings, once per status (once a second); nothing here allocates otherwise.

mod hud;
mod keys;
mod menu;
mod quit;

use std::collections::BTreeMap;

use klipolo_model::coded::{CodedError, ErrorParam, render};
use klipolo_model::time::Us;
use klipolo_platform::power::{Activity, begin_capture};

use crate::english::ENGLISH;
use crate::shell::{StatusView, WorkerState};

pub use hud::{FLASH_FOR, HudFrame, HudView, REDRAW_EVERY, gate_answer};
pub use keys::{HotkeyRegistry, KeysController, shortcut_text};
pub use menu::{
    MenuItem, action_text, dock_actions, menu_actions, menu_items, menu_lines, menu_shortcut,
    ready_line,
};
pub use quit::{QUIT_IDLE_WAIT, QUIT_OVERDUE_CODE, QUIT_TAKE_WAIT, quit_wait};

/// The item's title: the state's mark and the take's length, "● 12:34", "❚❚ 12:34", "● 1:02:03". Stopping shows the
/// length alone (it is no longer recording), and nothing for a take cancelled before the gate (no "0:00"); before the
/// gate, and after the take, there is no title (the icon alone).
pub fn status_title(view: &StatusView) -> String {
    let mark = match view.state {
        WorkerState::Recording => "● ",
        WorkerState::Paused => "❚❚ ",
        WorkerState::Stopping if view.elapsed_us > 0 => "",
        _ => return String::new(),
    };
    format!("{mark}{}", clock(view.elapsed_us))
}

/// The person's words: `capture.*` code → the UI's template in their language, with `{name}` placeholders (from
/// `SessionConfig::words`). A code it doesn't word, or words as nothing, reads in this crate's English (`english.rs`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Words(BTreeMap<String, String>);

impl Words {
    /// `code`'s words with each `{name}` replaced by its param; a placeholder without one stays as it is, and an unknown
    /// code comes back as it is.
    pub fn say(&self, code: &str, params: &[(&str, String)]) -> String {
        let mut e = CodedError::new(code);
        for (name, value) in params {
            e.params
                .insert((*name).to_owned(), ErrorParam::Text(value.clone()));
        }
        self.word(&e)
    }

    /// A coded message in these words, its nested codes too (a source's `capture.source.*` name, a
    /// `capture.duration.*` length), every value worded as `klipolo_model::coded::english` words it (one renderer,
    /// `coded::render`, so a time or a moment never reads differently here).
    pub fn word(&self, e: &CodedError) -> String {
        render(e, &|code| self.template(code))
    }

    /// The session's template for `code`, else this crate's English (a template worded as nothing counts as none).
    fn template(&self, code: &str) -> Option<&str> {
        (self.0.get(code))
            .map(String::as_str)
            .filter(|t| !t.is_empty())
            .or_else(|| ENGLISH.iter().find(|(c, _)| *c == code).map(|(_, m)| *m))
    }
}

impl From<BTreeMap<String, String>> for Words {
    fn from(words: BTreeMap<String, String>) -> Self {
        Self(words)
    }
}

impl<const N: usize> From<[(String, String); N]> for Words {
    fn from(words: [(String, String); N]) -> Self {
        Self(BTreeMap::from(words))
    }
}

/// The take's power activity (`begin_capture`), held while a take the person started runs: begun at Recording (or
/// Paused), kept through Stopping while the files are finished, and let go once the take ends. Never begun while armed,
/// starting or counting down, nor by a Cancel before Record, which goes through Stopping too. A
/// shell gives it every status's state.
#[derive(Default)]
pub struct TakeActivity {
    held: Option<Activity>,
}

impl TakeActivity {
    pub fn apply(&mut self, state: WorkerState) {
        match state {
            WorkerState::Recording | WorkerState::Paused if self.held.is_none() => {
                self.held = Some(begin_capture("Recording"));
            }
            WorkerState::Recording | WorkerState::Paused | WorkerState::Stopping => {}
            _ => self.held = None,
        }
    }

    pub fn held(&self) -> bool {
        self.held.is_some()
    }
}

/// Whether the take's camera bubble shows now: the person's "Show my camera while recording" is on for this
/// take (`on`, `SessionConfig::camera_bubble`), its camera is live, and the take counts down or records. It closes as
/// the take pauses, stops or loses the camera, and comes back on resume or reconnect. A fullscreen app's Space never
/// shows it (the shells keep it off fullscreen Spaces).
pub fn bubble_shows(on: bool, state: WorkerState, camera_live: bool) -> bool {
    on && camera_live && matches!(state, WorkerState::Countdown | WorkerState::Recording)
}

/// A length as a clock: `m:ss`, or `h:mm:ss` from an hour.
fn clock(us: Us) -> String {
    let s = us.max(0) / 1_000_000;
    let (h, m, s) = (s / 3_600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}
