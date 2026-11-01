//! Google Fonts' switch and the person's one consent, in `google-fonts.json` in Klipolo's support folder. Its own file,
//! so a change to the other settings never puts it back. A file that is missing, unreadable or not what Klipolo writes
//! reads as the defaults: on, not yet consented.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The file's name in the support folder.
pub const FILE: &str = "google-fonts.json";
/// The most of the file read: it is a few dozen bytes.
const MOST: u64 = 64 * 1024;

/// Google Fonts' switch (Settings) and whether the person confirmed "Download from Google Fonts?" once. Rust refuses
/// every download and preview unless both are true, whatever the page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GoogleFonts {
    #[serde(default = "on")]
    pub enabled: bool,
    #[serde(default)]
    pub consented: bool,
}

fn on() -> bool {
    true
}

impl Default for GoogleFonts {
    fn default() -> Self {
        Self {
            enabled: true,
            consented: false,
        }
    }
}

/// The setting saved in `dir`; the defaults when there is none Klipolo can read.
pub fn load(dir: &Path) -> GoogleFonts {
    let read = std::fs::File::open(dir.join(FILE)).and_then(|file| {
        let mut bytes = Vec::new();
        io::Read::read_to_end(&mut io::Read::take(file, MOST), &mut bytes)?;
        Ok(bytes)
    });
    read.ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter(serde_json::Value::is_object)
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// Saves `google` in `dir` (made when needed), whole or not at all: written beside, then renamed over the old file.
pub fn save(dir: &Path, google: &GoogleFonts) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let body = serde_json::to_vec(google).map_err(io::Error::other)?;
    let beside = dir.join(format!(".{FILE}.{}", std::process::id()));
    std::fs::write(&beside, body)?;
    std::fs::rename(&beside, dir.join(FILE)).inspect_err(|_| {
        let _ = std::fs::remove_file(&beside);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_fonts_start_on_and_not_consented_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), GoogleFonts::default());
        assert!(load(dir.path()).enabled);
        let saved = GoogleFonts {
            enabled: false,
            consented: true,
        };
        save(dir.path(), &saved).unwrap();
        assert_eq!(load(dir.path()), saved);
        // Not what Klipolo writes: the defaults.
        std::fs::write(dir.path().join(FILE), b"[true, true]").unwrap();
        assert_eq!(load(dir.path()), GoogleFonts::default());
        std::fs::write(dir.path().join(FILE), b"{\"consented\": true}").unwrap();
        assert_eq!(
            load(dir.path()),
            GoogleFonts {
                enabled: true,
                consented: true
            }
        );
    }
}
