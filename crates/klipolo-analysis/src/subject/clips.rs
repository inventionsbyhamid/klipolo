//! Main clips and the speaker: which file and stretch a clip plays (`footage`), the formats whose Fill crops it
//! (`cropped_formats`), its path from a tracked stretch (`clip_path`) and the op that makes it follow (`follow_op`).
//!
//! An imported video plays its file in its own time. A camera take (its camera the main footage) plays its camera
//! run's file: a clip's source time is the take's, its file time the run's own (`SourceRun::local_us`), so its path is
//! moved onto the take's clock. A screen or game take follows the cursor instead, and has no footage here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use klipolo_engine::plan::fill_crop;
use klipolo_model::fill_focus::{FillFocus, FocusPath};
use klipolo_model::framing::effective_framing;
use klipolo_model::ids::Id;
use klipolo_model::ops::{ClipPatch, Op};
use klipolo_model::project::{AssetKind, FormatKind, Project};
use klipolo_model::time::{RangeUs, Us};
use klipolo_model::timeline::{Clip, ClipSource, FramingMode};

use super::SubjectTrack;
use super::path::{KEY_STEP_US, PathInput, focus_path};

/// A Fill crop at least this share of the picture on both sides crops nothing.
const WHOLE: f64 = 0.999;

/// Where a main clip's footage comes from: a video file and the stretch the clip plays of it, on the file's clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Footage {
    pub asset_id: Id,
    pub path: String,
    pub range: RangeUs,
    /// The clip's source time is the file's time plus this.
    pub shift_us: Us,
}

impl Footage {
    pub fn file(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }
}

/// `clip`'s footage: an imported video's stretch, or a camera take's camera run's (the run playing at the clip's start,
/// cut to its end). None for a screen or game take, a picture, or a take with no camera run there.
pub fn footage(p: &Project, clip: &Clip) -> Option<Footage> {
    let (asset_id, range, shift_us) = match &clip.source {
        ClipSource::Asset { id } => (
            id.clone(),
            RangeUs {
                start: clip.source_in_us,
                end: clip.source_out_us,
            },
            0,
        ),
        ClipSource::Recording { id } => {
            let rec = p.recording(id).filter(|r| r.camera_only())?;
            let source = rec.primary_source()?;
            let (run, start) = source.run_at(clip.source_in_us)?;
            let (_, run_end) = run.rec_span(source.nudge_us);
            let end = run.local_us(clip.source_out_us.min(run_end), source.nudge_us);
            (
                run.asset_id.clone(),
                RangeUs { start, end },
                clip.source_in_us - start,
            )
        }
    };
    let asset = p.asset(&asset_id).filter(|a| a.kind == AssetKind::Video)?;
    (range.end > range.start).then(|| Footage {
        asset_id,
        path: asset.path.clone(),
        range,
        shift_us,
    })
}

/// Which of `formats` crop `clip`'s footage when it fills them: where its Fill crop is narrower or shorter than the
/// picture. A 16:9 video fills 16:9 whole, so only a vertical or square version has anywhere to follow.
pub fn cropped_formats(p: &Project, clip: &Clip, formats: &[FormatKind]) -> Vec<FormatKind> {
    formats
        .iter()
        .copied()
        .filter(|f| fill_crop(p, clip, *f).is_some_and(|c| c.w < WHOLE || c.h < WHOLE))
        .collect()
}

/// `clip`'s speaker path in `format` from `track` (which covers its footage), in the clip's source time, its keys kept
/// to what the clip plays (from the key holding its first frame to a key's step past its end). None when nobody was found, or Fill crops nothing there.
pub fn clip_path(
    p: &Project,
    clip: &Clip,
    format: FormatKind,
    footage: &Footage,
    track: &SubjectTrack,
) -> Option<FocusPath> {
    let crop = fill_crop(p, clip, format)?;
    let cuts: Vec<Us> = track.cuts.iter().map(|c| c + footage.shift_us).collect();
    let input = PathInput {
        frames: &track.frames,
        fps: track.fps,
        start_us: track.range.start + footage.shift_us,
        end_us: track.range.end + footage.shift_us,
        cuts: &cuts,
        crop: (crop.w, crop.h),
    };
    let path = focus_path(&input)?;
    let to = clip.source_out_us + KEY_STEP_US;
    // The last key at or before the clip's start holds its first frame.
    let first = path
        .keys
        .partition_point(|k| k.at_us <= clip.source_in_us)
        .max(1)
        - 1;
    let mut keys: Vec<_> = (path.keys.into_iter().enumerate())
        .filter(|(i, k)| *i >= first && k.at_us <= to)
        .map(|(_, k)| k)
        .collect();
    if let Some(k) = keys.first_mut() {
        k.cut = false;
    }
    Some(FocusPath { keys })
}

/// The op that makes `clip` follow its speaker in each of `paths`' formats: Fill there (the framing's own focus point
/// kept for Fixed), with the path as its focus. Other formats keep their framing and focus.
pub fn follow_op(p: &Project, clip: &Clip, paths: &BTreeMap<FormatKind, FocusPath>) -> Op {
    let mut framing = clip.framing.clone();
    let mut focus = clip.fill_focus.clone();
    for (format, path) in paths {
        let mut own = effective_framing(p, clip, *format);
        own.mode = FramingMode::Fill;
        framing.insert(*format, own);
        focus.insert(*format, FillFocus::Speaker { path: path.clone() });
    }
    Op::UpdateClip {
        id: clip.id.clone(),
        patch: ClipPatch {
            framing: Some(framing),
            fill_focus: Some(focus),
            ..ClipPatch::default()
        },
    }
}
