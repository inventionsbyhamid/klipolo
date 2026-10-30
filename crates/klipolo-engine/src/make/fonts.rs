//! The worker's fonts: configured from `in/fonts.json` once, before its first frame and before
//! its device, so its frames and measurements use the faces the preview uses. The parent writes that file once per
//! worker, before the launch: the app's `FontDirs` (the bundled folder) and the faces it downloaded (the managed font
//! store, readable through the sandbox's font folders). `configure` runs once and `register` once for the
//! downloads; the font system is `SharedFonts::new(false)`: the bundled and downloaded faces are there at once, and the
//! system scan runs in the background, so a request never waits on it.

use std::io::Read;
use std::path::{Path, PathBuf};

use klipolo_fonts::cjk::CjkCover;
use klipolo_fonts::index::{Downloaded, DownloadedFace};
use klipolo_fonts::resolve::FaceFile;
use serde::{Deserialize, Serialize};

use crate::render::fonts::{FontDirs, configure, register};
use crate::render::shared_fonts::SharedFonts;

/// The file's name in `in/` (no input may replace it: `InputName` refuses it).
pub const FONTS_NAME: &str = "fonts.json";
const MAX_FONTS_BYTES: u64 = 256 << 10;
/// Downloaded families one worker registers, at most.
const MAX_FAMILIES: usize = 256;

/// What `fonts.json` says.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerFonts {
    /// The bundled fonts folder; None for a broken install.
    pub bundled: Option<PathBuf>,
    #[serde(default)]
    pub downloaded: Vec<WorkerFamily>,
}

/// A downloaded Google family as its manifest describes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerFamily {
    pub family: String,
    pub version: String,
    pub faces: Vec<WorkerFace>,
    #[serde(default)]
    pub han: bool,
    #[serde(default)]
    pub kana: bool,
    #[serde(default)]
    pub hangul: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerFace {
    pub weight: u16,
    #[serde(default)]
    pub italic: bool,
    pub path: PathBuf,
    #[serde(default)]
    pub index: u32,
}

impl WorkerFonts {
    /// This process's fonts as the engine has them (the app writes this for its worker).
    pub fn of_this_process() -> Self {
        Self {
            bundled: crate::render::fonts::dirs().bundled,
            downloaded: Vec::new(),
        }
    }
}

/// Writes `fonts` as `session_in/fonts.json` (the parent, before the launch): whole or not at all.
pub fn write_fonts(session_in: &Path, fonts: &WorkerFonts) -> std::io::Result<()> {
    let temp = session_in.join(format!(".new-{FONTS_NAME}"));
    std::fs::write(
        &temp,
        serde_json::to_vec(fonts).map_err(std::io::Error::other)?,
    )?;
    std::fs::rename(&temp, session_in.join(FONTS_NAME))
}

/// Reads `session_in/fonts.json`, configures the engine's fonts once from it, registers its downloads, and makes the
/// worker's font system. A missing or unreadable file leaves the default folders (`bundled_dir()`'s order).
pub fn configure_worker_fonts(session_in: &Path) -> SharedFonts {
    match read(&session_in.join(FONTS_NAME)) {
        Ok(fonts) => {
            if let Err(e) = configure(FontDirs {
                bundled: fonts.bundled,
            }) {
                crate::make::say!("render worker: {e}");
            }
            let downloads: Vec<Downloaded> = fonts
                .downloaded
                .into_iter()
                .take(MAX_FAMILIES)
                .map(|f| Downloaded {
                    family: f.family,
                    version: f.version,
                    faces: f
                        .faces
                        .into_iter()
                        .map(|face| DownloadedFace {
                            weight: face.weight,
                            italic: face.italic,
                            file: FaceFile {
                                path: face.path,
                                index: face.index,
                            },
                        })
                        .collect(),
                    cover: CjkCover {
                        han: f.han,
                        kana: f.kana,
                        hangul: f.hangul,
                    },
                })
                .collect();
            if !downloads.is_empty() {
                register(downloads);
            }
        }
        Err(e) => crate::make::say!(
            "render worker: its fonts weren't given ({e}); it draws with the default folders"
        ),
    }
    SharedFonts::new(false)
}

fn read(path: &Path) -> Result<WorkerFonts, String> {
    let mut text = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_FONTS_BYTES + 1).read_to_end(&mut text))
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > MAX_FONTS_BYTES {
        return Err("too large".into());
    }
    serde_json::from_slice(&text).map_err(|e| e.to_string())
}
