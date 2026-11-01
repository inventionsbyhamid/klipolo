//! Which of a project's files analysis is for: the videos and sounds a main-track clip plays. Only main-track clips
//! feed captions, ducking (`speech_ranges`) and chat, and music, sound effects and pictures go on layers, never on the
//! main track, so a song, a whoosh or a file imported and never placed is neither offered nor paid for. The app's
//! analysis commands ask this, and the page's offer and notes ask `analysedAssets` in
//! `apps/desktop/src/lib/analysed.ts`, which mirrors it (pinned by `analysed-parity.json`, `tests/analysed.rs`).

use std::collections::HashSet;

use crate::ids::Id;
use crate::project::{Asset, AssetKind, Project};
use crate::timeline::ClipSource;

/// The assets analysis covers in `p`, in the project's order: every video or sound file a main-track clip plays (an
/// imported clip's own file, and each file of a recording's sources).
pub fn analysed_assets(p: &Project) -> Vec<&Asset> {
    let mut played: HashSet<&Id> = HashSet::new();
    for clip in &p.timeline.main {
        match &clip.source {
            ClipSource::Asset { id } => {
                played.insert(id);
            }
            ClipSource::Recording { id } => {
                if let Some(recording) = p.recording(id) {
                    played.extend(recording.asset_ids());
                }
            }
        }
    }
    p.assets
        .iter()
        .filter(|a| matches!(a.kind, AssetKind::Video | AssetKind::Audio) && played.contains(&a.id))
        .collect()
}

/// Whether analysis hears `asset_id` for its words only, never watching it for a scene log: a file of one of `p`'s
/// recordings (in any run of its sources), or the same file (by its path) imported again. A take's picture is the
/// person's screen or face; what it says is its voice's words. The page's `recordingOf` mirrors it (pinned by
/// `voices-parity.json`, `tests/analysed.rs`).
pub fn words_only(p: &Project, asset_id: &str) -> bool {
    let path = p.asset(asset_id).map(|a| a.path.as_str());
    p.recordings.iter().flat_map(|r| r.asset_ids()).any(|id| {
        id == asset_id || (path.is_some() && p.asset(id).map(|a| a.path.as_str()) == path)
    })
}
