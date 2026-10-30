//! The screen activity pass: each screen file a project shows turned into its activity
//! track (`klipolo_cursor::activity`: where the picture changed every 100 ms, never what it showed), in the media cache
//! by file key (`<media cache>/activity/<stem>.activity`). After import, in the background, after the sound cache and
//! the thumbnails, and never while the video plays, an export runs or a take records (`store`); one file's pass
//! resumes by 60-second chunk and stops within a tick (`run`); the decoder is the platform's (`platform::activity`).
//! An export makes the chunks it needs first, in the foreground (`ActivityStore::ensure`).
//!
//! The tracks are on each file's own clock (a take's later screen files start `SourceRun::offset_us` into it): their
//! readers add the offset. A file the pass can't read is planned without its activity; nothing waits for it.

mod files;
mod lru;
mod run;
mod store;

use std::collections::{HashMap, HashSet};

use klipolo_model::project::{Project, SourceRole};
use klipolo_model::timeline::ClipSource;

pub use store::{ActivityStore, MEMORY_BYTES};

/// The screen files the main track plays (each run of every screen source of the recordings it shows), each once, in
/// timeline order: the files whose activity the editor wants. O(clips + runs).
pub fn wanted(p: &Project) -> Vec<String> {
    let paths: HashMap<&str, &str> = p
        .assets
        .iter()
        .map(|a| (a.id.as_str(), a.path.as_str()))
        .collect();
    let recordings: HashMap<&str, _> = p.recordings.iter().map(|r| (r.id.as_str(), r)).collect();
    let (mut seen, mut listed) = (HashSet::new(), HashSet::new());
    let mut out = Vec::new();
    for clip in &p.timeline.main {
        let ClipSource::Recording { id } = &clip.source else {
            continue;
        };
        if !seen.insert(id.as_str()) {
            continue;
        }
        let Some(recording) = recordings.get(id.as_str()) else {
            continue;
        };
        let screens = recording
            .sources
            .iter()
            .filter(|s| s.role == SourceRole::Screen);
        for run in screens.flat_map(|s| &s.runs) {
            if let Some(&path) = paths.get(run.asset_id.as_str())
                && listed.insert(path)
            {
                out.push(path.to_owned());
            }
        }
    }
    out
}
