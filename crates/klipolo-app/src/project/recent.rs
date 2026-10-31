//! The start screen's recent projects: the library's project files (`~/Movies/Klipolo/*.klipolo`), the ones opened here
//! first, last opened first (stamped in `recent.json` in the app's data folder), then the rest by their last save.
//! Listing reads each file (`library::read`) and the engine's thumbnail cache and nothing else: it never opens a
//! project and never decodes a frame.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use klipolo_model::project::Project;
use klipolo_model::query::main_track_duration;
use klipolo_model::time::Us;
use klipolo_model::timeline::ClipSource;
use klipolo_platform::durable::{Durability, write_atomic};
use serde::Serialize;
use ts_rs::TS;

use super::PROJECT_EXTENSION;
use super::library::read;

/// A project file in the library, as the start screen shows it.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectRow {
    /// The project file, for `project_open`.
    pub path: String,
    pub name: String,
    /// When it was last opened (ms since the Unix epoch); its last save for a file never opened here.
    pub opened_ms: u64,
    /// The main track's length.
    pub duration_us: Us,
    /// Clips on the main track.
    pub clips: u32,
    /// Its first clip's first cached thumbnail; None until the clip has been scanned.
    pub thumb: Option<ThumbTile>,
}

/// The first tile of a sprite sheet of thumbnails: the top left `tile_w` by `tile_h` of the JPEG at `sprite` (read
/// through the asset protocol).
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThumbTile {
    pub sprite: String,
    pub tile_w: u32,
    pub tile_h: u32,
    /// Tiles per sprite row.
    pub cols: u32,
}

/// The most projects the start screen lists.
pub(crate) const RECENT_LIMIT: usize = 30;

/// The stamps' file in the app's data folder: `{ "<project file>": <opened ms> }`.
pub(crate) const RECENT_FILE: &str = "recent.json";

/// The newest `limit` projects in `library` (its top-level project files) as the start screen lists them: the ones
/// stamped in `stamps` first, by when they were opened, then the rest by their last save, which is then their
/// `opened_ms`. A file that doesn't read as a project is left out (and logged). `thumbs` is the engine's thumbnail
/// folder; None lists no pictures.
pub(crate) fn recent_in(
    library: &Path,
    stamps: &Path,
    thumbs: Option<&Path>,
    limit: usize,
) -> Vec<ProjectRow> {
    let stamps = read_stamps(stamps);
    project_files(library, &stamps)
        .into_iter()
        .filter_map(|found| match read(&found.file) {
            Ok(project) => Some(row(&found.file, found.at_ms, &project, thumbs)),
            Err(error) => {
                let name = found.file.file_name().unwrap_or_default().to_string_lossy();
                tracing::warn!(?error, %name, "a project in the library couldn't be read; it isn't listed");
                None
            }
        })
        .take(limit)
        .collect()
}

/// How many opened projects `recent.json` remembers.
const STAMPS_KEPT: usize = 50;

/// Stamps `file` in `stamps` as opened at `now_ms` (again, if it was), keeping the `STAMPS_KEPT` latest; written
/// atomically. A file that didn't read starts over.
pub(super) fn stamp(stamps: &Path, file: &Path, now_ms: u64) -> io::Result<()> {
    let mut opened = read_stamps(stamps);
    opened.insert(file.to_string_lossy().into_owned(), now_ms);
    if opened.len() > STAMPS_KEPT {
        let mut oldest_first: Vec<(u64, String)> = opened
            .iter()
            .map(|(file, &ms)| (ms, file.clone()))
            .collect();
        oldest_first.sort_unstable();
        let over = opened.len() - STAMPS_KEPT;
        for (_, file) in oldest_first.into_iter().take(over) {
            opened.remove(&file);
        }
    }
    let json = serde_json::to_vec(&opened).map_err(io::Error::other)?;
    if let Some(dir) = stamps.parent() {
        fs::create_dir_all(dir)?;
    }
    write_atomic(stamps, &json, Durability::Ordered)
}

/// Now, in ms since the Unix epoch.
pub(super) fn now_ms() -> u64 {
    ms_since_epoch(SystemTime::now())
}

/// A project file in the library and when it was opened here (`stamped`), else last saved.
struct Found {
    file: PathBuf,
    stamped: bool,
    at_ms: u64,
}

/// The top-level project files in `library`, in list order: stamped first, then newest first (by file name on a tie).
fn project_files(library: &Path, stamps: &BTreeMap<String, u64>) -> Vec<Found> {
    let entries = match fs::read_dir(library) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(%error, "the projects folder couldn't be read");
            return Vec::new();
        }
    };
    let mut found: Vec<Found> = entries
        .flatten()
        .filter_map(|entry| {
            let file = entry.path();
            if file.extension()? != PROJECT_EXTENSION {
                return None;
            }
            let meta = fs::metadata(&file).ok().filter(fs::Metadata::is_file)?;
            let stamp = stamps.get(file.to_string_lossy().as_ref()).copied();
            Some(Found {
                stamped: stamp.is_some(),
                at_ms: stamp.unwrap_or_else(|| meta.modified().map_or(0, ms_since_epoch)),
                file,
            })
        })
        .collect();
    found.sort_by(|a, b| list_order(a).cmp(&list_order(b)));
    found
}

/// Stamped first, then newest first, then by file name.
fn list_order(found: &Found) -> (Reverse<bool>, Reverse<u64>, &Path) {
    (Reverse(found.stamped), Reverse(found.at_ms), &found.file)
}

/// `time` in ms since the Unix epoch (0 before it).
fn ms_since_epoch(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
    })
}

/// The stamps in `stamps`; none when it is missing or doesn't read (logged).
fn read_stamps(stamps: &Path) -> BTreeMap<String, u64> {
    let bytes = match fs::read(stamps) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return BTreeMap::new(),
        Err(error) => {
            tracing::warn!(%error, "the recent projects couldn't be read");
            return BTreeMap::new();
        }
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        tracing::warn!(%error, "the recent projects aren't readable");
        BTreeMap::new()
    })
}

/// `project`, read from `file`, as a row.
fn row(file: &Path, opened_ms: u64, project: &Project, thumbs: Option<&Path>) -> ProjectRow {
    let main = &project.timeline.main;
    ProjectRow {
        path: file.to_string_lossy().into_owned(),
        name: project.name.clone(),
        opened_ms,
        duration_us: main_track_duration(main),
        clips: u32::try_from(main.len()).unwrap_or(u32::MAX),
        thumb: thumbs.and_then(|root| first_tile(root, first_file(project)?)),
    }
}

/// The file the first main clip shows: its file, or a recording's main footage (its primary source's first file).
fn first_file(project: &Project) -> Option<&str> {
    let asset_id = match &project.timeline.main.first()?.source {
        ClipSource::Asset { id } => id,
        ClipSource::Recording { id } => {
            &project
                .recording(id)?
                .primary_source()?
                .runs
                .first()?
                .asset_id
        }
    };
    project.asset(asset_id).map(|asset| asset.path.as_str())
}

/// The first tile of `file`'s thumbnails in the cache at `root` (chunk 0), once a scan has written it.
fn first_tile(root: &Path, file: &str) -> Option<ThumbTile> {
    let strip = klipolo_engine::media::thumbs::strip(root, file)?;
    let chunk = strip.chunks.into_iter().find(|chunk| chunk.index == 0)?;
    Some(ThumbTile {
        sprite: chunk.path,
        tile_w: strip.tile_w,
        tile_h: strip.tile_h,
        cols: strip.cols,
    })
}

#[cfg(test)]
#[path = "recent_tests.rs"]
mod tests;
