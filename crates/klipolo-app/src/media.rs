//! Media import: probe files and hand back assets the UI places with an op. One file that can't be read never stops
//! the rest; each skipped file is named with why, in plain words.

use std::path::{Path, PathBuf};

use klipolo_engine::render::probe::{MediaInfo, MediaKind, probe_media};
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::time::secs_to_us;
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
use ts_rs::TS;

/// File types Klipolo imports.
pub(crate) const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "webm", "avi", "mp3", "wav", "m4a", "aac", "flac", "ogg", "png",
    "jpg", "jpeg", "webp", "gif",
];

const MAX_FILES: usize = 100;
const TOO_MANY: &str = "Only 100 files can be added at once.";
const UNSUPPORTED: &str = "This file type isn't supported.";
const UNREADABLE: &str = "This file couldn't be read.";

/// The file's canonical path, if it is a regular local file.
fn local_file(path: &Path) -> Result<PathBuf, String> {
    let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    if !path.is_file() {
        return Err("not a regular file".into());
    }
    Ok(path)
}

/// Probes a local file in-process (the bundled libavformat; never an ffprobe process) into an asset. It keeps the
/// probed frame rate, so opening the file's decoder never probes it again.
pub(crate) fn probe_asset(path: &Path) -> Result<Asset, String> {
    let path = local_file(path)?;
    let info = probe_media(&path)?;
    Ok(asset_from_info(&path, info))
}

/// A new asset for the file at `path` (canonical) as probed, its rotation, rate and sound lead included.
pub(crate) fn asset_from_info(path: &Path, info: MediaInfo) -> Asset {
    let kind = match info.kind {
        MediaKind::Video => AssetKind::Video,
        MediaKind::Audio => AssetKind::Audio,
        MediaKind::Image => AssetKind::Image,
    };
    // Photos store the size as shown: a portrait phone photo is stored sideways with an EXIF orientation, and its
    // layout box must have the portrait shape the engine draws it in.
    let (width, height) = match kind {
        AssetKind::Image => klipolo_engine::render::image_load::display_size(path)
            .unwrap_or((info.width, info.height)),
        _ => (info.width, info.height),
    };
    Asset {
        id: uuid::Uuid::new_v4().to_string(),
        kind,
        path: path.to_string_lossy().into_owned(),
        name: file_name(path),
        duration_us: secs_to_us(info.duration),
        width,
        height,
        fps: info.fps,
        rotation: (kind == AssetKind::Video).then_some(info.rotation),
        has_audio: info.has_audio,
        // Where its sound starts against its picture, so its own sound plays in step with it. Sound files and images
        // have none.
        audio_lead_us: (kind == AssetKind::Video).then_some(info.audio_lead_us),
        made_with: None,
    }
}

/// What an import brought in, and the files it left out.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MediaImport {
    pub assets: Vec<Asset>,
    pub skipped: Vec<SkippedFile>,
}

/// A file that wasn't imported, by name (never its folder), with the reason in plain words.
#[derive(Debug, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SkippedFile {
    pub name: String,
    pub reason: String,
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn skipped(path: &Path, reason: &str) -> SkippedFile {
    SkippedFile {
        name: file_name(path),
        reason: reason.into(),
    }
}

/// Whether `path` has one of the media extensions Klipolo imports (`MEDIA_EXTENSIONS`, any case).
pub(crate) fn supported(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| MEDIA_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Probes each file on its own, in the order given, so one bad file never stops the rest.
pub(crate) fn probe_each(
    paths: Vec<PathBuf>,
    probe: impl Fn(&Path) -> Result<Asset, String>,
) -> MediaImport {
    let mut result = MediaImport {
        assets: Vec::new(),
        skipped: Vec::new(),
    };
    for (index, path) in paths.iter().enumerate() {
        if index >= MAX_FILES {
            result.skipped.push(skipped(path, TOO_MANY));
        } else if !supported(path) {
            result.skipped.push(skipped(path, UNSUPPORTED));
        } else {
            match probe(path) {
                Ok(asset) => result.assets.push(asset),
                Err(error) => {
                    tracing::info!(%error, file = %file_name(path), "a file couldn't be imported");
                    result.skipped.push(skipped(path, UNREADABLE));
                }
            }
        }
    }
    result
}

/// Probes `paths` (dropped from Finder), or the files the person picks when `paths` is null. The UI may then read the
/// imported files through the asset protocol.
#[tauri::command]
pub async fn media_import(
    app: AppHandle,
    paths: Option<Vec<String>>,
) -> Result<MediaImport, String> {
    let chosen: Vec<PathBuf> = match paths {
        Some(paths) => paths.into_iter().map(PathBuf::from).collect(),
        None => {
            let picker = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                picker
                    .dialog()
                    .file()
                    .set_title("Add videos, music, sounds or images")
                    .add_filter("Media", MEDIA_EXTENSIONS)
                    .blocking_pick_files()
            })
            .await
            .map_err(|e| e.to_string())?
            .unwrap_or_default()
            .into_iter()
            .filter_map(|file| file.into_path().ok())
            .collect()
        }
    };
    let probed = tauri::async_runtime::spawn_blocking(move || probe_each(chosen, probe_asset))
        .await
        .map_err(|e| e.to_string())?;
    let scope = app.asset_protocol_scope();
    let mut result = MediaImport {
        assets: Vec::with_capacity(probed.assets.len()),
        skipped: probed.skipped,
    };
    for asset in probed.assets {
        if scope.allow_file(&asset.path).is_ok() {
            result.assets.push(asset);
        } else {
            result
                .skipped
                .push(skipped(Path::new(&asset.path), UNREADABLE));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
