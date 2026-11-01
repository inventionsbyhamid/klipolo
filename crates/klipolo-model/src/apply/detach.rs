//! `DetachSource` (it generalises `DetachCamera`): one of a recording clip's sources becomes layers of its own,
//! one per run of it inside the clip, attached to the clip. The camera becomes muted video layers with the bubble's
//! look and mirror, and the bubble hides in every format; a sound source becomes sound layers, and the clip's own mix
//! of it is muted so it isn't heard twice. Layer ids are `{clip}-{source}`, then `-2`, `-3` (derived, so repeating
//! the edit repeats them). With the batch's indexes every step is O(runs × log layers) or O(media), so separating
//! 1,500 cameras in one batch is linear.
//!
//! `DetachSound` does the same for an imported video clip's own sound: one sound layer `{clip}-sound` over the clip,
//! and the clip muted.

use crate::error::{invalid, not_found, Result};
use crate::ids::Id;
use crate::layout::{Anchor, BoxN, Layout};
use crate::messages::code;
use crate::motion;
use crate::ops::TrackTarget;
use crate::project::{Project, RecordingSource, SourceMix, SourceRole};
use crate::query::{run_in_clip, RunInClip};
use crate::time::{scaled, Us, MIN_DURATION_US};
use crate::timeline::{
    Attach, CameraLayout, CameraMode, CameraShape, Clip, ClipSource, Item, ItemContent, Tie,
    ROUNDED_CAMERA_RADIUS,
};
use crate::transitions::split;

use super::fresh_id;
use super::item_ops::{add, taken};
use super::places::Places;

const NO_CAMERA: &str = code!("model.camera.none");

/// Separates source `source_id` of recording clip `clip_id` (see the module). Refused for the main footage, for a
/// source already playing in a layer of its own over the clip, for a sound source at a speed other than 1×, and when
/// the source recorded nothing during the clip. Returns the new layers' ids.
pub(super) fn detach_source(
    p: &mut Project,
    clip_id: &str,
    source_id: &str,
    places: &mut Places,
) -> Result<Vec<Id>> {
    let i = places
        .clip(&p.timeline.main, clip_id)
        .ok_or_else(|| not_found("clip", clip_id))?;
    let clip = &p.timeline.main[i];
    if clip.is_freeze() {
        return Err(invalid(code!("model.freeze.separate")));
    }
    let recording = match &clip.source {
        ClipSource::Recording { id } => p.recording(id),
        ClipSource::Asset { .. } => None,
    };
    let source = recording
        .and_then(|r| r.source(source_id))
        .ok_or_else(|| invalid(code!("model.source.missing")))?;
    if recording.is_some_and(|r| r.primary == source.id) {
        return Err(invalid(code!("model.source.primaryDetach")));
    }
    let camera = source.role == SourceRole::Camera;
    if camera && clip.camera.is_none() {
        return Err(invalid(NO_CAMERA));
    }
    let sound = !source.role.is_video();
    if sound && clip.speed != 1.0 {
        return Err(invalid(code!("model.source.detachSpeed")));
    }
    // Already separate: a layer of one of the source's files plays somewhere over the clip.
    let clip_start = places.start(&p.timeline.main, i);
    let clip_end = clip_start.saturating_add(clip.duration_us());
    if source
        .runs
        .iter()
        .any(|r| places.file_shown(&p.timeline.tracks, &r.asset_id, clip_start, clip_end))
    {
        return Err(invalid(if camera {
            code!("model.camera.alreadySeparate")
        } else {
            code!("model.source.alreadySeparate")
        }));
    }
    let mut layers = if sound {
        sound_layers(p, clip, clip_start, edge_fades(&p.timeline.main, i), source)
    } else {
        video_layers(p, clip, clip_start, source)
    };
    // The layers are the source's own: its sync moves them.
    let tie = recording.map(|r| Tie {
        recording: r.id.clone(),
        source: source.id.clone(),
    });
    for (item, _) in &mut layers {
        item.content.set_tie(tie.clone());
    }
    if layers.is_empty() {
        return Err(invalid(if camera {
            code!("model.camera.noFootage")
        } else {
            code!("model.source.noFootage")
        }));
    }
    let base = format!("{clip_id}-{}", source.id);
    let muted = sound.then(|| SourceMix {
        muted: true,
        ..clip.mix_for(source).clone()
    });
    let c = &mut p.timeline.main[i];
    match muted {
        // Heard from its layers now, not twice.
        Some(mix) => {
            c.source_mix.insert(source_id.to_owned(), mix);
        }
        None if camera => {
            if let Some(cam) = c.camera.as_mut() {
                cam.mode = CameraMode::Hidden;
                for mode in cam.mode_overrides.values_mut() {
                    *mode = CameraMode::Hidden;
                }
            }
        }
        None => {}
    }
    let mut ids = Vec::with_capacity(layers.len());
    for (mut item, file) in layers {
        item.id = fresh_id(base.clone(), |id| taken(p, id, places));
        let (start, end) = (item.start_us, item.end_us());
        ids.push(item.id.clone());
        add(p, &TrackTarget::Auto, item, places)?;
        places.file_added(&file, start, end);
    }
    Ok(ids)
}

/// Separates imported video clip `clip_id`'s own sound (`DetachSound`): one sound layer of its file over the whole
/// clip, attached at its in point, at the clip's volume (within the model's 200 %), mute and edge fades, so nothing
/// heard changes; the clip is muted. Refused for a recording clip (its sounds go one by one), a file without sound, a
/// speed other than 1×, and a clip whose file already plays in a sound layer over it. Returns the new layer's id.
pub(super) fn detach_sound(p: &mut Project, clip_id: &str, places: &mut Places) -> Result<Vec<Id>> {
    let i = places
        .clip(&p.timeline.main, clip_id)
        .ok_or_else(|| not_found("clip", clip_id))?;
    let clip = &p.timeline.main[i];
    if clip.is_freeze() {
        return Err(invalid(code!("model.freeze.separate")));
    }
    let ClipSource::Asset { id: file } = &clip.source else {
        return Err(invalid(code!("model.sound.recording")));
    };
    if !p.asset(file).is_some_and(|a| a.has_audio) {
        return Err(invalid(code!("model.sound.none")));
    }
    if clip.speed != 1.0 {
        return Err(invalid(code!("model.source.detachSpeed")));
    }
    let start = places.start(&p.timeline.main, i);
    let end = start.saturating_add(clip.duration_us());
    let heard_already = p.timeline.tracks.iter().flat_map(|t| &t.items).any(|item| {
        matches!(&item.content, ItemContent::Audio { asset_id, .. } if asset_id == file)
            && item.start_us < end
            && item.end_us() > start
    });
    if heard_already {
        return Err(invalid(code!("model.sound.alreadySeparate")));
    }
    let (fade_in, fade_out) = edge_fades(&p.timeline.main, i);
    let duration = end - start;
    let fade_in = fade_in.clamp(0, duration);
    let id = fresh_id(format!("{clip_id}-sound"), |id| taken(p, id, places));
    let item = Item {
        id: id.clone(),
        start_us: start,
        duration_us: duration,
        attach: anchor(clip, clip.source_in_us),
        layout: whole_frame(),
        content: ItemContent::Audio {
            asset_id: file.clone(),
            source_in_us: clip.source_in_us,
            volume: clip.volume.min(2.0),
            muted: clip.muted,
            fade_in_us: fade_in,
            fade_out_us: fade_out.clamp(0, duration - fade_in),
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
        },
        effects: Vec::new(),
    };
    // Heard from its layer now, not twice.
    p.timeline.main[i].muted = true;
    add(p, &TrackTarget::Auto, item, places)?;
    Ok(vec![id])
}

/// Where each run of `source` plays inside `clip`, at least 10 ms of it, no further than its file reaches: the run's
/// file, its span, and the recording time the span starts at.
fn spans<'a>(
    p: &Project,
    clip: &Clip,
    clip_start: Us,
    source: &'a RecordingSource,
) -> Vec<(&'a str, f64, RunInClip, Us)> {
    source
        .runs
        .iter()
        .filter_map(|run| {
            let mut span = run_in_clip(clip, clip_start, source, run)?;
            let rate = 1.0 + run.drift_ppm * 1e-6;
            if let Some(asset) = p.asset(&run.asset_id) {
                let footage = asset.duration_us.saturating_sub(span.run_in_us);
                let reach = scaled((footage as f64 / rate).round() as Us, clip.speed);
                span.end_us = span.end_us.min(span.start_us.saturating_add(reach));
            }
            let from = run.rec_span(source.nudge_us).0.max(clip.source_in_us);
            (span.end_us.saturating_sub(span.start_us) >= MIN_DURATION_US).then_some((
                run.asset_id.as_str(),
                run.drift_ppm,
                span,
                from,
            ))
        })
        .collect()
}

/// Anchored in source time at the clip moment where the layer's footage starts.
fn anchor(clip: &Clip, from: Us) -> Option<Attach> {
    Some(Attach {
        clip_id: clip.id.clone(),
        source: clip.source.clone(),
        source_us: from,
    })
}

/// A video source's layers: muted video at the clip's speed, with the bubble's place, shape and keys (moved from the
/// recording's source time into each layer's own time) when the clip has one; a whole-frame layer otherwise. A camera
/// keeps the bubble's mirror, so separating it never flips the person.
fn video_layers(p: &Project, clip: &Clip, start: Us, source: &RecordingSource) -> Vec<(Item, Id)> {
    let mirror =
        source.role == SourceRole::Camera && clip.camera.as_ref().is_some_and(|c| c.mirror);
    spans(p, clip, start, source)
        .into_iter()
        .map(|(file, _, span, from)| {
            let duration = span.end_us - span.start_us;
            let layout = match &clip.camera {
                Some(camera) => bubble_layout(camera, clip, from, duration),
                None => whole_frame(),
            };
            let item = Item {
                id: String::new(),
                start_us: span.start_us,
                duration_us: duration,
                attach: anchor(clip, from),
                layout,
                // Muted (the mic carries the voice); unmuting plays the camera's own sound.
                content: ItemContent::Video {
                    asset_id: file.to_owned(),
                    source_in_us: span.run_in_us,
                    speed: clip.speed,
                    volume: 1.0,
                    muted: true,
                    mirror,
                    tie: None,
                },
                effects: Vec::new(),
            };
            (item, file.to_owned())
        })
        .collect()
}

/// The bubble as a layer: its shape, and its keys from `from` on, in the layer's own time.
fn bubble_layout(camera: &CameraLayout, clip: &Clip, from: Us, duration: Us) -> Layout {
    let mut layout = camera.bubble.clone();
    layout.motion = layout.motion.take().and_then(|mut m| {
        m.trim_start(from, Us::MAX);
        m.retime(|t| scaled(t, clip.speed), duration);
        motion::kept(m)
    });
    layout.radius = match camera.shape {
        CameraShape::Circle => 0.5,
        CameraShape::Rounded if layout.radius > 0.0 => layout.radius,
        CameraShape::Rounded => ROUNDED_CAMERA_RADIUS,
        CameraShape::Square => 0.0,
    };
    layout
}

/// The fades main clip `i`'s sound has at its edges (the audio plan's): its own fade-in and fade-out, or, across a
/// transition, the part of the transition's window inside the clip (a layer stays inside its clip, so it fades over
/// that part instead of crossfading past the cut).
fn edge_fades(main: &[Clip], i: usize) -> (Us, Us) {
    let clip = &main[i];
    let incoming = i
        .checked_sub(1)
        .and_then(|k| main[k].transition_out.as_ref());
    let outgoing = main.get(i + 1).and(clip.transition_out.as_ref());
    (
        incoming.map_or(clip.fade_in_us, |t| split(t.duration_us).1),
        outgoing.map_or(clip.fade_out_us, |t| split(t.duration_us).0),
    )
}

/// A sound source's layers: its file as the clip played it, drifting as its run does. The clip played the source at
/// `clip.volume × mix.volume`, and not at all when either was muted (the audio plan), so the layer takes that gain
/// (within the model's 200 %) and mute, its mix's audio effects, all of them, in order, with their settings and whether
/// each is on, and the channel it plays centred from: nothing heard changes. A layer that reaches one of the clip's
/// edges takes the clip's fade there (`fades`) as its own (sync must hold), kept within the layer's length; a run that
/// starts or ends inside the clip has none at that end, as in the audio plan.
fn sound_layers(
    p: &Project,
    clip: &Clip,
    start: Us,
    fades: (Us, Us),
    source: &RecordingSource,
) -> Vec<(Item, Id)> {
    let mix = clip.mix_for(source);
    let volume = (clip.volume * mix.volume).min(2.0);
    let muted = clip.muted || mix.muted;
    let end = start.saturating_add(clip.duration_us());
    spans(p, clip, start, source)
        .into_iter()
        .map(|(file, drift_ppm, span, from)| {
            let duration = span.end_us - span.start_us;
            let fade_in = if span.start_us == start { fades.0 } else { 0 }.clamp(0, duration);
            let fade_out =
                if span.end_us == end { fades.1 } else { 0 }.clamp(0, duration - fade_in);
            let item = Item {
                id: String::new(),
                start_us: span.start_us,
                duration_us: duration,
                attach: anchor(clip, from),
                layout: whole_frame(),
                content: ItemContent::Audio {
                    asset_id: file.to_owned(),
                    source_in_us: span.run_in_us,
                    volume,
                    muted,
                    fade_in_us: fade_in,
                    fade_out_us: fade_out,
                    effect: None,
                    drift_ppm,
                    centre_from: mix.centre_from,
                    tie: None,
                    duck: None,
                },
                effects: mix.effects.clone(),
            };
            (item, file.to_owned())
        })
        .collect()
}

/// Sound layers have no picture: their layout is the whole frame.
fn whole_frame() -> Layout {
    Layout::new(
        Anchor::Center,
        BoxN {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
    )
}
