//! What the timeline draws on its blocks: a file's waveform peaks (binary, by range and level) and its thumbnail
//! strip, from the engine's caches in the app's cache folder (`audio/` and `media/thumbs/`). Only files of the open
//! project are served. These are read-only queries for the page, never edits.

use std::path::{Path, PathBuf};

use klipolo_engine::audio::pcm::peaks::{self, PeaksInfo};
use klipolo_engine::audio::pcm::peaks_file;
use klipolo_engine::media::thumbs::{self, ThumbStrip};
use tauri::State;

use crate::project::{ProjectSessionState, uses_path};
use crate::state::EngineState;

const NOT_IN_PROJECT: &str = "That file isn't part of this project.";
const NOT_READY: &str = "The waveform isn't ready yet.";

/// A file's waveform levels and length, once the engine has written its peaks (None until then).
#[tauri::command(async)]
pub fn media_peaks_info(
    engine: State<'_, EngineState>,
    project: State<'_, ProjectSessionState>,
    path: String,
) -> Result<Option<PeaksInfo>, String> {
    known(&project, &path)?;
    Ok(peaks_path(engine.cache_dir.as_deref(), &path).and_then(|p| peaks::info(&p)))
}

/// Raw little-endian i16 `[min, max]` per channel per bucket of `level`, from bucket `from` (at most 16,384).
#[tauri::command(async)]
pub fn media_peaks(
    engine: State<'_, EngineState>,
    project: State<'_, ProjectSessionState>,
    path: String,
    level: u32,
    from: u64,
    count: u32,
) -> Result<tauri::ipc::Response, String> {
    known(&project, &path)?;
    let file = peaks_path(engine.cache_dir.as_deref(), &path).ok_or(NOT_READY)?;
    let bytes = peaks::read(&file, level, from, count).map_err(|e| e.to_string())?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// A file's thumbnail strip: its finished sprite chunks (JPEGs the page reads through the asset protocol).
#[tauri::command(async)]
pub fn media_thumbs(
    engine: State<'_, EngineState>,
    project: State<'_, ProjectSessionState>,
    path: String,
) -> Result<Option<ThumbStrip>, String> {
    known(&project, &path)?;
    Ok(engine
        .cache_dir
        .as_deref()
        .and_then(|cache| thumbs::strip(&thumbs_root(cache), &path)))
}

/// Where the engine keeps thumbnails (`with_media_cache`): the cache's `media/thumbs`.
fn thumbs_root(cache: &Path) -> PathBuf {
    cache.join("media").join("thumbs")
}

/// The finished peaks file of `path` in the sound cache (`with_audio_cache`: the cache's `audio`), when there is one.
fn peaks_path(cache: Option<&Path>, path: &str) -> Option<PathBuf> {
    peaks_file(&cache?.join("audio"), path)
}

fn known(project: &ProjectSessionState, path: &str) -> Result<(), String> {
    if uses_path(project, path) {
        Ok(())
    } else {
        Err(NOT_IN_PROJECT.into())
    }
}

#[cfg(test)]
mod tests {
    use klipolo_model::project::{Asset, AssetKind, Project};

    use super::*;
    use crate::project::opened;

    fn video(path: &str) -> Asset {
        Asset {
            id: "a".into(),
            kind: AssetKind::Video,
            path: path.into(),
            name: "a.mp4".into(),
            duration_us: 1_000_000,
            width: 1280,
            height: 720,
            fps: 30.0,
            rotation: None,
            has_audio: true,
            audio_lead_us: None,
        }
    }

    #[test]
    fn only_the_open_projects_files_are_read() {
        let none = ProjectSessionState::default();
        assert_eq!(
            known(&none, "/media/a.mp4"),
            Err(NOT_IN_PROJECT.into()),
            "nothing is open"
        );
        let mut project = Project::new("Media");
        project.assets.push(video("/media/a.mp4"));
        let state = opened(project);
        assert_eq!(known(&state, "/media/a.mp4"), Ok(()));
        assert_eq!(known(&state, "/etc/passwd"), Err(NOT_IN_PROJECT.into()));
    }

    #[test]
    fn media_is_read_from_the_engines_caches() {
        // Where `start_engine`'s `with_audio_cache` and `with_media_cache` put them.
        assert_eq!(
            thumbs_root(Path::new("/cache")),
            Path::new("/cache/media/thumbs")
        );
        assert_eq!(
            peaks_path(None, "/media/a.mp4"),
            None,
            "no cache folder: no waveform"
        );
    }
}
