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
use super::path::{PathInput, focus_path};
use crate::providers::TrackedFrame;

/// A Fill crop at least this share of the picture on both sides crops nothing.
const WHOLE: f64 = 0.999;
/// The tracked frames read either side of a clip's footage.
const MARGIN_US: Us = 1_000_000;

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

/// Which of `formats` `clip` may follow its speaker in: where its Fill crops the footage and it is on Fill (its own
/// framing, else the default). A format the person set to Fit is never turned to Fill by a follow.
pub fn followable_formats(p: &Project, clip: &Clip, formats: &[FormatKind]) -> Vec<FormatKind> {
    (cropped_formats(p, clip, formats).into_iter())
        .filter(|f| effective_framing(p, clip, *f).mode == FramingMode::Fill)
        .collect()
}

/// `clip`'s speaker path in `format` from the tracked stretches of its file (`tracks`, from one or several requests,
/// in any order), over just the footage it plays, in its source time. None when nobody was found there, or Fill crops
/// nothing.
pub fn clip_path(
    p: &Project,
    clip: &Clip,
    format: FormatKind,
    footage: &Footage,
    tracks: &[SubjectTrack],
) -> Option<FocusPath> {
    // Fill's crop is in what the clip's crop leaves; the path is in the whole file.
    let kept = clip.crop.unwrap_or_default().kept();
    let crop = fill_crop(p, clip, format)?;
    let crop = (crop.w * kept.w, crop.h * kept.h);
    let shift = footage.shift_us;
    let (from, to) = (footage.range.start, footage.range.end);
    let mut frames: Vec<(Us, &TrackedFrame)> = Vec::new();
    let mut cuts: Vec<Us> = Vec::new();
    for t in tracks
        .iter()
        .filter(|t| t.range.start < to && from < t.range.end)
    {
        let fps = if t.fps > 0.0 { t.fps } else { 1.0 };
        frames.extend((t.frames.iter()).filter_map(|f| {
            let at = t.range.start + (f64::from(f.frame) / fps * 1e6).round() as Us;
            // A second either side, so the crop eases in toward the clip's first and last frames.
            (from - MARGIN_US..to + MARGIN_US)
                .contains(&at)
                .then_some((at + shift, f))
        }));
        cuts.extend(
            t.cuts
                .iter()
                .filter(|c| (from..to).contains(*c))
                .map(|c| c + shift),
        );
    }
    frames.sort_by_key(|(at, _)| *at);
    frames.dedup_by_key(|(at, _)| *at);
    cuts.sort_unstable();
    cuts.dedup();
    let input = PathInput {
        frames: &frames,
        start_us: from + shift,
        end_us: to + shift,
        cuts: &cuts,
        crop,
    };
    focus_path(&input)
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
