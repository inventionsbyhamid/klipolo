//! Per-game profiles (UX decision 9): choosing a game remembers how it was recorded (capture mode, quality, sound,
//! cursor, auto-pause, the HUD's corner and, on Linux, the portal's restore token), so the next take of that game starts
//! the same way. The app keeps them at `<app data>/recorder/profiles.json`: at most 200 games, the least recently
//! updated dropped first. Settings, not media: a missing or damaged file starts over with none.

use std::io;
use std::path::Path;

use klipolo_platform::durable::{Durability, write_atomic};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::backend::PresetId;

/// How a game is captured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CaptureMode {
    /// The game's windows on its display.
    #[default]
    App,
    Window,
    /// The whole display without Klipolo's own windows.
    DisplayWithoutKlipolo,
}

/// Where the HUD flashes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Corner {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GameProfile {
    /// The game's `GameInfo::app_id`: a bundle id (macOS), an executable path (Windows), a desktop-file id or
    /// executable (Linux).
    pub app_id: String,
    /// The game's name as its OS shows it (user content).
    pub name: String,
    pub capture: CaptureMode,
    /// The quality the person picked for this game. None: "Best for this computer", the probe's choice
    /// (`probe::Choices::screen`), so a new profile never asks for more than this computer can do. The preflight checks
    /// stored preset against `Choices::available` (it may be stale after an eGPU is unplugged) and falls back to None.
    pub preset: Option<PresetId>,
    /// Record the game's sound.
    pub sound: bool,
    /// The game's cursor is recorded, burned into its frames (`preflight::cursor::cursor_plan`: a game never gets a
    /// separate cursor); off: none at all, the default (games usually draw their own).
    pub cursor: bool,
    /// "Pause while the game isn't in front" (hidden where `CaptureCaps::focus_events` is false).
    pub auto_pause: bool,
    pub hud_corner: Corner,
    /// Linux's ScreenCast portal token that restores this game's last pick without the picker (`Target::Portal`). It is
    /// single-use: the pipe's `PipeEvent::RestoreToken` replaces it on every start.
    pub restore_token: Option<String>,
    /// When it was last chosen or changed, in ms since 1970.
    pub updated_ms: u64,
}

impl Default for GameProfile {
    /// A game's defaults (UX decisions 2 and 9): its app's windows at the quality best for this computer, with its sound
    /// and auto-pause on, no cursor, and the HUD top right.
    fn default() -> Self {
        Self {
            app_id: String::new(),
            name: String::new(),
            capture: CaptureMode::default(),
            preset: None,
            sound: true,
            cursor: false,
            auto_pause: true,
            hud_corner: Corner::default(),
            restore_token: None,
            updated_ms: 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profiles {
    pub games: Vec<GameProfile>,
}

impl Profiles {
    pub const MAX: usize = 200;

    /// A missing or damaged file gives none (logged).
    pub fn load(path: &Path) -> Self {
        let read = std::fs::read(path).and_then(|bytes| {
            serde_json::from_slice::<Self>(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        });
        match read {
            Ok(mut profiles) => {
                profiles.trim(None);
                profiles
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                tracing::debug!(path = %path.display(), "game profiles: none saved yet");
                Self::default()
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "game profiles: unreadable, starting with none");
                Self::default()
            }
        }
    }

    /// Replaces the file whole (`write_atomic`: a crash leaves the old profiles or the new ones), making its folder.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => std::fs::create_dir_all(dir)?,
            _ => {}
        }
        write_atomic(path, &serde_json::to_vec_pretty(self)?, Durability::Ordered)
    }

    pub fn get(&self, app_id: &str) -> Option<&GameProfile> {
        self.games.iter().find(|g| g.app_id == app_id)
    }

    /// Replaces the game's profile, or adds it; past `MAX` the least recently updated other game goes.
    pub fn put(&mut self, p: GameProfile) {
        let at = match self.games.iter().position(|g| g.app_id == p.app_id) {
            Some(i) => {
                self.games[i] = p;
                i
            }
            None => {
                self.games.push(p);
                self.games.len() - 1
            }
        };
        self.trim(Some(at));
    }

    /// Drops the least recently updated games (the list's order breaks ties) until `MAX` are left, never `keep`.
    fn trim(&mut self, mut keep: Option<usize>) {
        while self.games.len() > Self::MAX {
            let oldest = (0..self.games.len())
                .filter(|&i| Some(i) != keep)
                .min_by_key(|&i| self.games[i].updated_ms);
            let Some(oldest) = oldest else { break };
            self.games.remove(oldest);
            keep = keep.map(|k| if oldest < k { k - 1 } else { k });
        }
    }
}
