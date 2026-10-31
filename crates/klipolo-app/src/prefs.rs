//! The app's own settings that are no secret, in `prefs.json` in Klipolo's support folder (`klipolo_mcp::connection::
//! support_dir`, which the stdio bridge reads too): for now, whether other apps may edit the open project
//! (`mcp_enabled`, off until the person turns it on). A file that is missing, unreadable or not what Klipolo writes
//! reads as the defaults.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

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
}

/// The settings saved in `dir`; the defaults (everything off) when there are none Klipolo can read.
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
