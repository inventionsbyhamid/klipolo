//! The session's global shortcuts on macOS (`MacHotkeys`): Carbon's `RegisterEventHotKey` through `global-hotkey`,
//! behind the shell view's `KeysController`, so they are registered only while the recorder is armed for a take. Each
//! binding registers on its own, so one that another app holds never keeps the others from working; the menu still
//! does what any of them would. Media keys are never bound: `global-hotkey` would watch them with an event tap, which
//! sees every key event and needs Input Monitoring (a prompt).

use std::str::FromStr;

use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::{Code, HotKey};
use klipolo_platform::caps::OsFamily;

use crate::shell::{HotkeyAction, KeyBindings};
use crate::shell_view::{HotkeyRegistry, shortcut_text};

/// The registered shortcuts: each with its action and the binding it came from.
pub(super) struct MacHotkeys {
    manager: Option<GlobalHotKeyManager>,
    held: Vec<(HotKey, HotkeyAction, String)>,
}

impl MacHotkeys {
    /// On the main thread: Carbon calls the handler on the application's event target.
    pub(super) fn new() -> Self {
        let manager = (GlobalHotKeyManager::new())
            .map_err(
                |error| tracing::warn!(%error, "shell: no global shortcuts; the menu still works"),
            )
            .ok();
        Self {
            manager,
            held: Vec::new(),
        }
    }

    /// The action a pressed shortcut (by `global-hotkey`'s id) stands for.
    pub(super) fn action(&self, id: u32) -> Option<HotkeyAction> {
        (self.held.iter())
            .find(|(key, ..)| key.id() == id)
            .map(|&(_, action, _)| action)
    }

    /// `binding` is registered: no other app holds it.
    pub(super) fn holds(&self, binding: &str) -> bool {
        self.held.iter().any(|(.., held)| held == binding)
    }
}

impl HotkeyRegistry for MacHotkeys {
    fn register(&mut self, keys: &KeyBindings) -> Result<(), String> {
        let mut taken = Vec::new();
        for (binding, action) in keys.all() {
            let registered = bindable(binding).and_then(|key| {
                let manager = (self.manager.as_ref()).ok_or("no shortcut manager")?;
                manager.register(key).map_err(|e| e.to_string())?;
                Ok(key)
            });
            match registered {
                Ok(key) => self.held.push((key, action, binding.to_owned())),
                Err(why) => {
                    tracing::warn!(binding, why, "shell: a shortcut didn't register");
                    taken.push(shortcut_text(binding, OsFamily::Macos));
                }
            }
        }
        if taken.is_empty() {
            Ok(())
        } else {
            Err(taken.join(", "))
        }
    }

    fn unregister(&mut self) {
        for (key, ..) in self.held.drain(..) {
            if let Some(manager) = &self.manager
                && let Err(error) = manager.unregister(key)
            {
                tracing::warn!(%error, "shell: a shortcut didn't unregister");
            }
        }
    }
}

/// `binding` (`KeyBindings`' syntax) parsed for Carbon; a media key is refused (the module doc).
pub(super) fn bindable(binding: &str) -> Result<HotKey, String> {
    let key = HotKey::from_str(binding).map_err(|e| e.to_string())?;
    if is_media(key.key) {
        return Err("a media key".into());
    }
    Ok(key)
}

/// Play, pause, next, previous, fast-forward, rewind, stop, record and select: the keys `global-hotkey` would watch
/// with an event tap.
fn is_media(code: Code) -> bool {
    code.to_string().starts_with("Media")
}

#[cfg(test)]
mod tests {
    use global_hotkey::hotkey::Modifiers;

    use super::*;

    #[test]
    fn the_defaults_parse_and_media_keys_are_never_bound() {
        let keys = KeyBindings::defaults(OsFamily::Macos);
        let start = bindable(&keys.start_stop).unwrap();
        assert_eq!(start.key, Code::KeyR);
        assert_eq!(start.mods, Modifiers::ALT | Modifiers::SUPER);
        assert_eq!(keys.start_over, None, "Start over has no default key");
        for media in ["MediaPlayPause", "Alt+Super+MediaTrackNext", "MediaStop"] {
            assert!(bindable(media).is_err(), "{media}");
        }
    }
}
