//! What the timeline draws on its blocks: a file's waveform peaks (binary, by range and level) and its thumbnail
//! strip, from the engine's caches in the app's cache folder (`state::CacheDirs`, the folders the engine writes). Only
//! files of the open project are served. These are read-only queries for the page, never edits.

use std::path::PathBuf;

use klipolo_engine::audio::pcm::peaks::{self, PeaksInfo};
use klipolo_engine::audio::pcm::peaks_file;
use klipolo_engine::media::thumbs::{self, ThumbStrip};
use tauri::State;

use crate::project::{ProjectSessionState, uses_path};
use crate::state::{CacheDirs, EngineState};

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
    Ok(peaks_path(engine.caches().as_ref(), &path).and_then(|p| peaks::info(&p)))
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
    let file = peaks_path(engine.caches().as_ref(), &path).ok_or(NOT_READY)?;
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
        .caches()
        .and_then(|caches| thumbs::strip(&caches.thumbs(), &path)))
}

/// The finished peaks file of `path` in the sound cache, when there is one.
fn peaks_path(caches: Option<&CacheDirs>, path: &str) -> Option<PathBuf> {
    peaks_file(&caches?.audio, path)
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

    /// A peaks file and a thumbnail chunk written the way the engine writes them, in the folders the app gives the
    /// engine, are what these commands read.
    #[test]
    fn media_is_read_from_the_folders_the_engine_writes() {
        use klipolo_engine::audio::pcm::peaks::Peaks;
        use klipolo_engine::media::key::{CacheKind, stem};
        use klipolo_engine::media::sprite::Sprite;
        use klipolo_engine::media::thumbs::Fill;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.mp4");
        std::fs::write(&file, b"not really a video").unwrap();
        let path = file.to_str().unwrap();
        let caches = CacheDirs::in_folder(&dir.path().join("cache"));
        assert_eq!(peaks_path(None, path), None, "no cache folder: no waveform");
        assert_eq!(peaks_path(Some(&caches), path), None, "not decoded yet");

        // The sound cache's peaks (`PcmStore` names them `<stem>.peaks` in its folder).
        std::fs::create_dir_all(&caches.audio).unwrap();
        let named = caches
            .audio
            .join(format!("{}.peaks", stem(path, &CacheKind::Pcm).unwrap()));
        let mut peaks = Peaks::create(&named, 1).unwrap();
        peaks.push(0, &[0.5; 512]).unwrap();
        peaks.finish(512).unwrap();
        let found = peaks_path(Some(&caches), path).expect("the engine's peaks are found");
        assert_eq!(peaks::info(&found).unwrap().channels, 1);

        // A thumbnail chunk, written as a scan writes it under the media cache.
        let thumbs_stem = stem(path, &CacheKind::Thumbs).unwrap();
        thumbs::write_chunk(
            &klipolo_platform::durable::Os,
            &caches.thumbs(),
            &thumbs_stem,
            &Sprite::new(0, (16, 10)),
            Fill::Keys,
        )
        .unwrap();
        assert!(thumbs::strip(&caches.thumbs(), path).is_some());
    }
}
