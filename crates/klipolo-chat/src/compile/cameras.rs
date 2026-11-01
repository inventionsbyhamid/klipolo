//! The cameras a camera tool acts on (`find`), and two of the tools:
//! - `setCameraMode`: the built-in cameras take the mode, in the format given (`mode_overrides`) or in every format
//!   (its overrides cleared). A split puts the camera under the screen in a vertical video, so without a format it is
//!   for portrait, which the video must have. A camera that is its own layer can only be hidden in every format, which
//!   hides its layer's track (`own_tracks`), or shown again as a bubble ("show my camera"), which shows that track
//!   again; another mode for it is refused, or noted when other cameras take the mode.
//! - `detachCamera`: one `DetachSource` of its camera per clip whose camera shows in every format; the model makes the
//!   layers. A camera that is its own layer already is left as it is, a hidden one stays in its clip (its layer would
//!   show it everywhere), one that recorded nothing during its clip stays too (the model would refuse the whole call),
//!   and a camera that is its clip's footage has nothing to separate: each is noted.

use std::collections::{BTreeMap, HashMap, HashSet};

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ids::{Id, new_id};
use klipolo_model::ops::{ClipPatch, Op, TrackPatch};
use klipolo_model::project::FormatKind;
use klipolo_model::time::{MIN_DURATION_US, Us};
use klipolo_model::timeline::{CameraMode, Clip, ClipSource, ItemContent};

use super::recorded::{Noted, clip_recording, has_format, named, noted};
use super::{Compiler, Edit, Refused, label, to_model, to_person};
use crate::tools::recorded::mode_word;

/// Said when a camera that is its own layer shows again as that layer.
pub const LAYER_SHOWN: &str = "That camera is its own layer, so its layer shows again.";
/// Said when cameras that are their own layer stay as they are while the others change.
pub const SEPARATE_LEFT: &str = "Cameras that are their own layer were left as they are.";
/// Said when the only cameras named are their own layer and the mode isn't hidden everywhere or a bubble.
pub const LAYER_FORMATS: &str =
    "That camera is its own layer now, so it can only be hidden in every format.";
pub const SEPARATE: &str = "That camera is its own layer now, so it can't go back into its clip.";
/// Said when separating a camera that is its own layer already.
pub const ALREADY_SEPARATE: &str = "That camera is already its own layer.";
/// Said when a camera recorded nothing during a clip it is asked to leave.
pub const NOTHING_RECORDED: &str =
    "A camera that recorded nothing during its clip was left as it is.";
/// Said when a camera hidden in some format is asked to become its own layer.
pub const HIDDEN_STAYS: &str =
    "A hidden camera stays hidden in its clip. Show it first to make it its own layer.";
/// Said when none of the clips in the video has a camera.
pub const NO_CAMERA: &str = "No clip in your video has a camera.";

/// The cameras a camera tool acts on, in timeline order.
#[derive(Debug, Default)]
pub(crate) struct Cameras {
    /// Clips that draw their own camera (a bubble or a split, maybe hidden).
    pub(crate) on_clips: Vec<Id>,
    /// Clips whose camera is a layer of its own now, with that layer (the one reaching furthest, if several play over
    /// the clip).
    pub(crate) separate: Vec<(Id, Id)>,
    /// Clips whose camera is their footage (a camera only take): they have no bubble, so only the mirror reaches them.
    pub(crate) footage: Vec<Id>,
}

impl Cameras {
    /// The separate cameras' layers, each once, in timeline order.
    pub(super) fn layers(&self) -> Vec<Id> {
        let mut seen = HashSet::new();
        (self.separate.iter())
            .filter(|(_, layer)| seen.insert(layer.as_str()))
            .map(|(_, layer)| layer.clone())
            .collect()
    }
}

/// `every_camera` of the clips `named` (None: every clip), refused when none of them has a camera: back to the model
/// when it named them, else to the person.
pub(super) fn find(
    work: &Project,
    tool: &str,
    named: Option<&HashSet<Id>>,
) -> Result<Cameras, Refused> {
    let cams = every_camera(work, named);
    if cams.on_clips.is_empty() && cams.separate.is_empty() && cams.footage.is_empty() {
        return Err(match named {
            Some(_) => to_model(format!("{tool}: none of those clips has a camera.")),
            None => to_person(NO_CAMERA.to_owned()),
        });
    }
    Ok(cams)
}

/// Every camera of the clips `named` (None: every clip) in `work`, in timeline order: on its clip, separate, or the
/// clip's footage. A clip's camera is separate once a video layer of its recording's camera files plays over the clip
/// (the model's own test before `DetachSource`): that layer is the camera now, and the clip's own stays hidden.
/// O(clips + layers), plus a binary search per clip into the camera layers sorted by start.
pub(crate) fn every_camera(work: &Project, named: Option<&HashSet<Id>>) -> Cameras {
    // Each recording's camera files (one per run of its camera; usually one).
    let camera_of: HashMap<&str, Vec<&str>> = (work.recordings.iter())
        .filter_map(|r| {
            let files = r.camera()?.runs.iter().map(|run| run.asset_id.as_str());
            Some((r.id.as_str(), files.collect()))
        })
        .collect();
    let camera_files: HashSet<&str> = camera_of.values().flatten().copied().collect();
    // Each camera file's layers by start, with the furthest end reached so far (and its layer) at each.
    let mut layers: HashMap<&str, Vec<(Us, Us, &str)>> = HashMap::new();
    for item in work.timeline.tracks.iter().flat_map(|t| &t.items) {
        if let ItemContent::Video { asset_id, .. } = &item.content
            && camera_files.contains(asset_id.as_str())
        {
            layers.entry(asset_id.as_str()).or_default().push((
                item.start_us,
                item.end_us(),
                item.id.as_str(),
            ));
        }
    }
    for spans in layers.values_mut() {
        spans.sort_unstable_by_key(|s| s.0);
        for k in 1..spans.len() {
            if spans[k - 1].1 > spans[k].1 {
                (spans[k].1, spans[k].2) = (spans[k - 1].1, spans[k - 1].2);
            }
        }
    }
    let mut cams = Cameras::default();
    let mut start: Us = 0;
    for clip in &work.timeline.main {
        let end = start.saturating_add(clip.duration_us());
        let wanted = named.is_none_or(|n| n.contains(&clip.id));
        if clip.camera.is_some() && wanted {
            let recording = match &clip.source {
                ClipSource::Recording { id } => work.recording(id),
                ClipSource::Asset { .. } => None,
            };
            if recording.is_some_and(|r| r.camera_only()) {
                cams.footage.push(clip.id.clone());
            } else {
                let files = recording.and_then(|r| camera_of.get(r.id.as_str()));
                let over = files.into_iter().flatten().find_map(|a| {
                    let spans = layers.get(a)?;
                    let before_end = spans.partition_point(|s| s.0 < end);
                    (before_end.checked_sub(1))
                        .map(|k| spans[k])
                        .filter(|s| s.1 > start)
                });
                match over {
                    Some((_, _, layer)) => {
                        cams.separate.push((clip.id.clone(), layer.to_owned()));
                    }
                    None => cams.on_clips.push(clip.id.clone()),
                }
            }
        }
        start = end;
    }
    cams
}

/// `cams`, when one of them is drawn by its clip or is a layer of its own; else the person is told the camera is the
/// video itself, which no bubble tool changes.
pub(super) fn bubbles(work: &Project, cams: Cameras) -> Result<Cameras, Refused> {
    if !cams.on_clips.is_empty() || !cams.separate.is_empty() {
        return Ok(cams);
    }
    Err(to_person(match cams.footage.first() {
        Some(clip) => no_bubble(work, clip),
        None => NO_CAMERA.to_owned(),
    }))
}

/// "Recording 1's camera is the video itself, so it has no bubble."
pub(super) fn no_bubble(work: &Project, clip: &str) -> String {
    format!(
        "{}'s camera is the video itself, so it has no bubble.",
        clip_recording(work, clip)
    )
}

/// The working copy's clips by id: built once a call, so a tool for every clip stays linear.
pub(super) fn clips_by_id(work: &Project) -> HashMap<&str, &Clip> {
    (work.timeline.main.iter())
        .map(|clip| (clip.id.as_str(), clip))
        .collect()
}

/// The track that layer `id` is on, by index.
fn track_of(work: &Project, id: &str) -> Option<usize> {
    (work.timeline.tracks.iter()).position(|t| t.items.iter().any(|i| i.id == id))
}

pub(super) fn set_mode(
    c: &Compiler,
    work: &Project,
    clips: Option<&[String]>,
    mode: CameraMode,
    format: Option<FormatKind>,
) -> Result<Noted, Refused> {
    const TOOL: &str = "setCameraMode";
    // A split puts the camera under the screen in a vertical video: without a format it is for the vertical version.
    let format = match (format, mode) {
        (None, CameraMode::SplitBottom) => Some(FormatKind::Portrait),
        (f, _) => f,
    };
    if let Some(f) = format {
        has_format(work, f)?;
    }
    let named = named(c, work, TOOL, clips)?;
    let cams = bubbles(work, find(work, TOOL, named.as_ref())?)?;
    let mut ops = Vec::new();
    let mut notes = Vec::new();
    if !cams.separate.is_empty() {
        let layers = cams.layers();
        let hidden = |id: &Id| track_of(work, id).is_some_and(|t| work.timeline.tracks[t].hidden);
        // "Show my camera": the separate cameras whose layer is hidden show again, as that layer.
        let shown: Vec<Id> = if mode == CameraMode::Bubble && format.is_none() {
            layers.iter().filter(|l| hidden(l)).cloned().collect()
        } else {
            Vec::new()
        };
        if mode == CameraMode::Hidden && format.is_none() {
            let showing: Vec<Id> = layers.iter().filter(|l| !hidden(l)).cloned().collect();
            ops.extend(own_tracks(work, &showing, true));
        } else if !shown.is_empty() {
            ops.extend(own_tracks(work, &shown, false));
            notes.push(LAYER_SHOWN.to_owned());
        } else if cams.on_clips.is_empty() {
            let said = if mode == CameraMode::Hidden {
                LAYER_FORMATS
            } else {
                SEPARATE
            };
            return Err(to_person(said.to_owned()));
        } else {
            notes.push(SEPARATE_LEFT.to_owned());
        }
    }
    let by_id = clips_by_id(work);
    for id in &cams.on_clips {
        let Some(cam) = by_id.get(id.as_str()).and_then(|clip| clip.camera.as_ref()) else {
            continue;
        };
        let mut now = cam.clone();
        match format {
            Some(f) => {
                now.mode_overrides.insert(f, mode);
            }
            None => {
                now.mode = mode;
                now.mode_overrides.clear();
            }
        }
        if now != *cam {
            ops.push(Op::UpdateClip {
                id: id.clone(),
                patch: ClipPatch {
                    camera: Some(now),
                    ..ClipPatch::default()
                },
            });
        }
    }
    let label = label(TOOL, vec![("mode", ErrorParam::from(mode_word(mode)))]);
    noted(Edit { ops, label }, notes)
}

/// Ops that hide (or show) the layers `ids` by their track: a track whose every layer is among them is hidden itself;
/// layers that share a track with others move together onto one new track of its kind just above it (their order
/// against every other track stays), which is hidden. Nothing about the layers changes, so showing them again restores
/// them exactly. Tracks are visited from the top down, so each new track's index is still right when it is added.
/// O(layers).
fn own_tracks(work: &Project, ids: &[Id], hidden: bool) -> Vec<Op> {
    let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
    // Per track (by index): its layers among `ids`, with their starts.
    let mut moving: BTreeMap<usize, Vec<(Id, Us)>> = BTreeMap::new();
    for (t, track) in work.timeline.tracks.iter().enumerate() {
        for item in (track.items.iter()).filter(|i| wanted.contains(i.id.as_str())) {
            (moving.entry(t).or_default()).push((item.id.clone(), item.start_us));
        }
    }
    let patch = TrackPatch {
        hidden: Some(hidden),
        ..TrackPatch::default()
    };
    let mut ops = Vec::new();
    for (t, layers) in moving.into_iter().rev() {
        let track = &work.timeline.tracks[t];
        if layers.len() == track.items.len() {
            ops.push(Op::UpdateTrack {
                id: track.id.clone(),
                patch: patch.clone(),
            });
            continue;
        }
        let new = new_id();
        ops.push(Op::AddTrack {
            id: Some(new.clone()),
            kind: track.kind,
            index: Some(t + 1),
        });
        for (id, start_us) in layers {
            ops.push(Op::MoveItem {
                id,
                track_id: Some(new.clone()),
                start_us,
            });
        }
        ops.push(Op::UpdateTrack {
            id: new,
            patch: patch.clone(),
        });
    }
    ops
}

pub(super) fn detach(
    c: &Compiler,
    work: &Project,
    clips: Option<&[String]>,
) -> Result<Noted, Refused> {
    const TOOL: &str = "detachCamera";
    let named = named(c, work, TOOL, clips)?;
    let cams = find(work, TOOL, named.as_ref())?;
    let mut notes = Vec::new();
    if !cams.separate.is_empty() {
        notes.push(ALREADY_SEPARATE.to_owned());
    }
    // A camera that is its clip's footage has nothing to separate: said, alone or beside what else is done.
    if let Some(clip) = cams.footage.first() {
        notes.push(format!(
            "{} is the camera itself, so there's nothing to separate.",
            clip_recording(work, clip)
        ));
    }
    let formats = &work.settings.formats;
    let (mut hidden, mut nothing) = (false, false);
    let mut ops = Vec::with_capacity(cams.on_clips.len());
    let by_id = clips_by_id(work);
    for clip_id in &cams.on_clips {
        let Some(clip) = by_id.get(clip_id.as_str()) else {
            continue;
        };
        // The recording's camera source: `DetachSource` separates it.
        let source = match &clip.source {
            ClipSource::Recording { id } => work.recording(id).and_then(|r| r.camera()),
            ClipSource::Asset { .. } => None,
        };
        let shows = (clip.camera.as_ref()).is_some_and(|cam| {
            formats
                .iter()
                .all(|f| cam.mode_for(*f) != CameraMode::Hidden)
        });
        // At least the model's shortest layer of its footage plays during the clip.
        let recorded = |s: &klipolo_model::project::RecordingSource| {
            s.runs.iter().any(|r| {
                let (from, to) = r.rec_span(s.nudge_us);
                to.min(clip.source_out_us) - from.max(clip.source_in_us) >= MIN_DURATION_US
            })
        };
        match source {
            Some(source) if shows && recorded(source) => ops.push(Op::DetachSource {
                clip_id: clip_id.clone(),
                source_id: source.id.clone(),
            }),
            Some(_) if shows => nothing = true,
            _ => hidden = true,
        }
    }
    if hidden {
        notes.push(HIDDEN_STAYS.to_owned());
    }
    if nothing {
        notes.push(NOTHING_RECORDED.to_owned());
    }
    if ops.is_empty() && notes.is_empty() {
        return Err(to_model(format!("{TOOL}: no camera there to separate.")));
    }
    noted(
        Edit {
            ops,
            label: label(TOOL, Vec::new()),
        },
        notes,
    )
}
