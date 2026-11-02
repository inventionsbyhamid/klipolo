//! Which of a project's files analysis is for: the videos and sounds a main-track clip plays. Only main-track clips
//! feed captions, ducking (`speech_ranges`) and chat, and music, sound effects and pictures go on layers, never on the
//! main track, so a song, a whoosh or a file imported and never placed is neither offered nor paid for. The app's
//! analysis commands ask this, and the page's offer and notes ask `analysedAssets` in
//! `apps/desktop/src/lib/analysed.ts`, which mirrors it (pinned by `analysed-parity.json`, `tests/analysed.rs`).

use std::collections::HashSet;

use crate::ids::Id;
use crate::project::{Asset, AssetKind, Project, RecordingSource, SourceRun};
use crate::time::Us;
use crate::timeline::{Clip, ClipSource};

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
/// recordings (in any run of its sources), or the same file (by its path) imported again, unless it is a camera take's
/// footage (`camera_footage_file`), which is watched as imported camera footage is. A screen take's picture is the
/// person's screen and its camera a bubble of their face; what it says is its voice's words. The page's `wordsOnly`
/// mirrors it (pinned by `voices-parity.json`, `tests/analysed.rs`).
pub fn words_only(p: &Project, asset_id: &str) -> bool {
    let ids = p.recordings.iter().flat_map(|r| r.asset_ids());
    held_by(p, ids, asset_id) && !camera_footage_file(p, asset_id)
}

/// Whether `asset_id` is the main footage of one of `p`'s camera takes ("Record yourself", or a take whose screen
/// didn't start: `Recording::camera_only`), in any run of its camera, or the same file (by its path) imported again.
/// It is watched for a scene log, and its people followed, as imported camera footage is.
pub fn camera_footage_file(p: &Project, asset_id: &str) -> bool {
    let footage = (p.recordings.iter())
        .filter(|r| r.camera_only())
        .filter_map(|r| r.primary_source())
        .flat_map(|s| s.runs.iter().map(|run| &run.asset_id));
    held_by(p, footage, asset_id)
}

/// Whether `ids` hold `asset_id`, or a file at the same path.
fn held_by<'p>(p: &'p Project, mut ids: impl Iterator<Item = &'p Id>, asset_id: &str) -> bool {
    let path = p.asset(asset_id).map(|a| a.path.as_str());
    ids.any(|id| id == asset_id || (path.is_some() && p.asset(id).map(|a| a.path.as_str()) == path))
}

/// One picture file of a camera take as a main-track clip plays it (`camera_footage`): a run of the take's camera, the
/// main footage, on the take's clock (the clip's source time).
#[derive(Clone, Copy, Debug)]
pub struct FootageRun<'p> {
    pub source: &'p RecordingSource,
    pub run: &'p SourceRun,
}

impl<'p> FootageRun<'p> {
    pub fn asset_id(&self) -> &'p Id {
        &self.run.asset_id
    }

    /// The clip's source time where the file plays its own time `file_us`.
    pub fn source_of(&self, file_us: Us) -> Us {
        self.run.rec_of(file_us, self.source.nudge_us)
    }

    /// The file's own time at the clip's source time `source_us`.
    pub fn file_of(&self, source_us: Us) -> Us {
        self.run.local_us(source_us, self.source.nudge_us)
    }

    /// Where the run plays on the clip's source time: [from, to).
    pub fn source_span(&self) -> (Us, Us) {
        self.run.rec_span(self.source.nudge_us)
    }
}

/// The runs of `clip`'s camera take's camera, its main footage, in order (`Recording::camera_only`); none for any other
/// clip (an imported file plays its own file in its own time, and a screen take's camera is a bubble).
pub fn camera_footage<'p>(p: &'p Project, clip: &Clip) -> Vec<FootageRun<'p>> {
    let ClipSource::Recording { id } = &clip.source else {
        return Vec::new();
    };
    let Some(source) = p
        .recording(id)
        .filter(|r| r.camera_only())
        .and_then(|r| r.primary_source())
    else {
        return Vec::new();
    };
    (source.runs.iter())
        .filter(|run| {
            p.asset(&run.asset_id)
                .is_some_and(|a| a.kind == AssetKind::Video)
        })
        .map(|run| FootageRun { source, run })
        .collect()
}
