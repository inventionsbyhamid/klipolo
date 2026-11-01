//! The app's own settings that are no secret, in `prefs.json` in Klipolo's support folder (`klipolo_mcp::connection::
//! support_dir`, which the stdio bridge reads too): whether other apps may edit the open project (`mcp_enabled`, off
//! until the person turns it on), and the theme Settings' Appearance picked (`theme`, the system's until one is). A file
//! that is missing, unreadable or not what Klipolo writes reads as the defaults.

use std::io;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

/// The file's name in the support folder.
pub const FILE: &str = "prefs.json";
/// The most of the file read: it is a few dozen bytes.
const MOST: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    /// "Let other apps edit with Klipolo": the MCP server listens on 127.0.0.1 while it is on.
    #[serde(default)]
    pub mcp_enabled: bool,
    /// Settings' Appearance. Read before the window shows, so its colour is the theme's from the start.
    #[serde(default, deserialize_with = "theme_or_system")]
    pub theme: Theme,
}

/// The theme the person picked: the system's appearance (the default), or always light or always dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// A theme this Klipolo doesn't know (a later version's) reads as the system's, and the other settings still read.
fn theme_or_system<'de, D: Deserializer<'de>>(value: D) -> Result<Theme, D::Error> {
    Ok(Theme::deserialize(value).unwrap_or_default())
}

/// The settings saved in `dir`; the defaults (everything off, the system's theme) when there are none Klipolo can read.
pub fn load(dir: &Path) -> Prefs {
    let read = std::fs::File::open(dir.join(FILE)).and_then(|file| {
        let mut bytes = Vec::new();
        io::Read::read_to_end(&mut io::Read::take(file, MOST), &mut bytes)?;
        Ok(bytes)
    });
    // An object, as Klipolo writes it: a list of values isn't read as the settings in order.
    read.ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter(serde_json::Value::is_object)
        .and_then(|prefs| serde_json::from_value(prefs).ok())
        .unwrap_or_default()
}

/// Saves `prefs` in `dir` (made when needed), whole or not at all: written beside, then renamed over the old file.
pub fn save(dir: &Path, prefs: &Prefs) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let body = serde_json::to_vec(prefs).map_err(io::Error::other)?;
    let beside = dir.join(format!(".{FILE}.{}", std::process::id()));
    std::fs::write(&beside, body)?;
    std::fs::rename(&beside, dir.join(FILE)).inspect_err(|_| {
        let _ = std::fs::remove_file(&beside);
    })
}

/// Changes one setting in `dir` and saves them all: the others stay as saved.
pub fn update(dir: &Path, change: impl FnOnce(&mut Prefs)) -> io::Result<()> {
    let mut prefs = load(dir);
    change(&mut prefs);
    save(dir, &prefs)
}
