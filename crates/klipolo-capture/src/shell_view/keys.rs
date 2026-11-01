//! The session's global shortcuts, registered only while the recorder is armed (a hotkey never
//! starts anything by itself).

use klipolo_model::coded::CodedError;
use klipolo_platform::caps::OsFamily;

use crate::english::msg;
use crate::shell::KeyBindings;

/// The OS's global hotkeys (Carbon, `RegisterHotKey`, the GlobalShortcuts portal, X11), behind `KeysController`.
pub trait HotkeyRegistry {
    /// Registers the session's bindings. `Err` names the ones that couldn't be (taken by another app), as the shell
    /// shows them ("⌥⌘M"); the others may well be registered (`global-hotkey` stops at the first it can't take).
    fn register(&mut self, keys: &KeyBindings) -> Result<(), String>;
    /// Releases every binding this registry holds, whichever registered.
    fn unregister(&mut self);
}

/// The session's bindings, registered only while the recorder is armed for a take: from `ShellOutput::Keys(true)` to
/// `Keys(false)`. Repeats change nothing. A registration that partly failed counts as registered, so `Keys(false)`
/// always releases what it took: the keys that work stay usable for the take, and none outlives
/// it.
pub struct KeysController<R: HotkeyRegistry> {
    registry: R,
    keys: KeyBindings,
    on: bool,
    problem: Option<CodedError>,
}

impl<R: HotkeyRegistry> KeysController<R> {
    pub fn new(registry: R, keys: KeyBindings) -> Self {
        Self {
            registry,
            keys,
            on: false,
            problem: None,
        }
    }

    pub fn apply(&mut self, armed: bool) {
        match (armed, self.on) {
            (true, false) => {
                self.on = true;
                if let Err(taken) = self.registry.register(&self.keys) {
                    tracing::warn!(taken, "shell: some hotkeys are taken by another app");
                    self.problem = Some(msg!("capture.keys.taken", keys = taken.as_str()));
                }
            }
            (false, true) => {
                self.registry.unregister();
                (self.on, self.problem) = (false, None);
            }
            _ => {}
        }
    }

    pub fn registry(&self) -> &R {
        &self.registry
    }

    /// The session's bindings.
    pub fn keys(&self) -> &KeyBindings {
        &self.keys
    }

    /// The shortcuts another app holds, as a line for the shell's menu (`capture.keys.taken`, worded with `Words::word`
    /// like every other line). The menu still does what they would.
    pub fn problem(&self) -> Option<&CodedError> {
        self.problem.as_ref()
    }
}

/// A binding in `KeyBindings`' syntax ("Alt+Super+KeyR") as `os` shows shortcuts: "⌥⌘R" on macOS, in its modifier
/// order (⌃⌥⇧⌘); "Ctrl+Alt+R" on Windows and Linux. A key code loses its `Key` or `Digit` prefix; any other key name
/// stays as written ("F5", "Space").
pub fn shortcut_text(binding: &str, os: OsFamily) -> String {
    let (mut ctrl, mut alt, mut shift, mut logo, mut key) = (false, false, false, false, "");
    for token in binding.split('+').map(str::trim) {
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            "super" | "cmd" | "command" | "meta" => logo = true,
            _ => key = token,
        }
    }
    let key = (key.strip_prefix("Key"))
        .or_else(|| key.strip_prefix("Digit"))
        .unwrap_or(key);
    let marks = match os {
        OsFamily::Macos => ["⌃", "⌥", "⇧", "⌘"],
        OsFamily::Windows => ["Ctrl+", "Alt+", "Shift+", "Win+"],
        OsFamily::Linux => ["Ctrl+", "Alt+", "Shift+", "Super+"],
    };
    let mut text = String::new();
    for (mark, held) in marks.into_iter().zip([ctrl, alt, shift, logo]) {
        if held {
            text.push_str(mark);
        }
    }
    text.push_str(key);
    text
}
