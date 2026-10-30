//! What the timeline draws on its blocks, derived from each source file: thumbnail sprites cut from keyframes by
//! a scan that runs after the file's sound is cached (sound first; `store::ThumbStore`, `scan`); the
//! keyframe index the preview decoders seek with, recorded by that scan (`keyframes`); and the events that tell the UI
//! when a file's waveform or filmstrip is ready (`events`); and editing copies of sources that scrub slowly (`proxy`,
//! (`proxy`), which also refill the filmstrip from a full decode. Every cache is named by `key::stem`. Also a
//! recording's cursor data, which Auto zooms follow (`cursor`), and where its screen changed (`activity`, after the
//! after the thumbnails).

pub mod activity;
pub mod cursor;
pub mod events;
pub mod frozen;
pub mod key;
pub mod keyframes;
pub mod open;
pub mod proxy;
pub mod scan;
pub mod sprite;
pub mod still;
pub mod still_png;
pub mod store;
pub mod thumbs;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::query::{clip_starts, run_in_clip};
use klipolo_model::time::Us;
use klipolo_model::timeline::{ClipSource, ItemContent};

pub use events::{MediaEvent, MediaKind, MediaSink};
pub use store::{Hold, ThumbStore, Want};

/// The video files a project shows (main clips' footage and cameras, overlay video), each once, the one under
/// `playhead_us` first, then in timeline order; images and sound-only files have no thumbnails.
pub fn wants(project: &Project, playhead_us: Us) -> Vec<Want> {
    // Runs on the actor for every edit: one pass over each list (an hour's project has thousands of clips).
    let videos: HashMap<&str, &Asset> = project
        .assets
        .iter()
        .filter(|a| a.kind == AssetKind::Video)
        .map(|a| (a.id.as_str(), a))
        .collect();
    let video = |id: &str| videos.get(id).copied();
    // (under the playhead, timeline start, want)
    let mut found: Vec<(bool, Us, Want)> = Vec::new();
    let mut add = |id: &str, start: Us, duration: Us, source_in: Us, speed: f64| {
        let Some(asset) = video(id) else { return };
        let under = start <= playhead_us && playhead_us < start.saturating_add(duration);
        let at_us = if under {
            source_in + ((playhead_us - start) as f64 * speed) as Us
        } else {
            source_in
        };
        found.push((
            under,
            start,
            Want {
                path: asset.path.clone(),
                duration_us: asset.duration_us,
                at_us,
            },
        ));
    };
    let main = &project.timeline.main;
    for (clip, start) in main.iter().zip(clip_starts(main)) {
        match &clip.source {
            ClipSource::Asset { id } => {
                add(id, start, clip.duration_us(), clip.source_in_us, clip.speed)
            }
            // Each run of the recording's video sources where it plays inside the clip, at its own time.
            ClipSource::Recording { id } => {
                let Some(r) = project.recording(id) else {
                    continue;
                };
                for source in r.sources.iter().filter(|s| s.role.is_video()) {
                    for run in &source.runs {
                        if let Some(span) = run_in_clip(clip, start, source, run) {
                            let length = span.end_us - span.start_us;
                            add(
                                &run.asset_id,
                                span.start_us,
                                length,
                                span.run_in_us,
                                clip.speed,
                            );
                        }
                    }
                }
            }
        }
    }
    for item in project.timeline.tracks.iter().flat_map(|t| &t.items) {
        if let ItemContent::Video {
            asset_id,
            source_in_us,
            speed,
            ..
        } = &item.content
        {
            add(
                asset_id,
                item.start_us,
                item.duration_us,
                *source_in_us,
                *speed,
            );
        }
    }
    // Stable: at the same start, main footage before its camera before overlays.
    found.sort_by_key(|(under, start, _)| (!under, *start));
    let mut seen = HashSet::new();
    found
        .into_iter()
        .map(|(_, _, w)| w)
        .filter(|w| seen.insert(w.path.clone()))
        .collect()
}

/// The source files of unfinished export jobs (`<exports>/.jobs/<id>/project.json`; a finished job's folder is
/// removed). A paused or interrupted export reads their sound again, so cache tidies keep them.
pub fn export_job_sources(jobs_root: &Path) -> HashSet<String> {
    #[derive(serde::Deserialize)]
    struct Snapshot {
        #[serde(default)]
        assets: Vec<Source>,
    }
    #[derive(serde::Deserialize)]
    struct Source {
        path: String,
    }
    let Ok(entries) = std::fs::read_dir(jobs_root) else {
        return HashSet::new();
    };
    entries
        .flatten()
        .filter(|e| crate::export::jobs::valid_job_id(&e.file_name().to_string_lossy()))
        .filter_map(|e| std::fs::read(e.path().join(crate::export::job::PROJECT_FILE)).ok())
        .filter_map(|bytes| serde_json::from_slice::<Snapshot>(&bytes).ok())
        .flat_map(|s| s.assets.into_iter().map(|a| a.path))
        .collect()
}
