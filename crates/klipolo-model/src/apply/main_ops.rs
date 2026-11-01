//! Main-track edits. Deletions ripple: gaps close and attached layers follow their content.
//!
//! Layers move by timeline lengths but are re-anchored in source time against the main track as
//! it was before the cut, so rounding at non-unit speeds can never hand a layer to a neighbouring
//! clip.
//!
//! Op-supplied times are checked against `0..=MAX_TIME_US` before use, and all other arithmetic
//! saturates, so even a corrupt project only ever fails validation.

use crate::error::{invalid, invalid_with, not_found, Result};
use crate::ids::Id;
use crate::messages::{code, msg};
use crate::motion::{self, Motion};
use crate::ops::{ClipPatch, Edge};
use crate::project::Project;
use crate::query::{anchor_at, clip_at, main_track_duration, MainIndex};
use crate::time::{
    drifted, kept_parts, merge_ranges, scaled, unscaled, RangeUs, Ripple, Us, MIN_DURATION_US,
};
use crate::timeline::{Attach, Clip, ClipSource, Item, ItemContent, Transition};
use crate::transitions::{max_transition_us, MIN_TRANSITION_US};

use super::generator_pieces;
use super::item_ops::{advance_source, fit_fades};
use super::places::Places;
use super::{check_time, insert_sorted, IdPool};

/// For each clip of the rebuilt main track: the index of the old clip it came from and the span
/// of the old timeline its content filled.
type Origins = Vec<(usize, RangeUs)>;

/// Where a rippled layer (or layer piece) now starts, in terms of the main track before the cut.
struct LayerStart {
    /// First moment of the old timeline the layer still covers.
    at: Us,
    /// The layer's anchor and the old clip playing it, when that moment is the anchor's own.
    anchor: Option<(usize, Attach)>,
}

/// The first clip with `id`, through the batch's index.
fn clip_index(p: &Project, id: &str, places: &mut Places) -> Result<usize> {
    places
        .clip(&p.timeline.main, id)
        .ok_or_else(|| not_found("clip", id))
}

/// Timeline start and end of `main[i]`.
fn clip_span(main: &[Clip], i: usize) -> (Us, Us) {
    let start = main_track_duration(&main[..i]);
    (start, start.saturating_add(main[i].duration_us()))
}

fn clamp_fades(c: &mut Clip) {
    let d = c.duration_us().max(0);
    c.fade_in_us = c.fade_in_us.clamp(0, d);
    c.fade_out_us = c.fade_out_us.clamp(0, d.saturating_sub(c.fade_in_us));
}

pub(super) fn insert_clip(p: &mut Project, clip: &Clip, index: Option<usize>) -> Result<()> {
    let len = p.timeline.main.len();
    let at = index.unwrap_or(len).min(len);
    p.timeline.main.insert(at, clip.clone());
    Ok(())
}

pub(super) fn move_clip(
    p: &mut Project,
    id: &str,
    index: usize,
    places: &mut Places,
) -> Result<()> {
    let from = clip_index(p, id, places)?;
    let clip = p.timeline.main.remove(from);
    let at = index.min(p.timeline.main.len());
    p.timeline.main.insert(at, clip);
    Ok(())
}

pub(super) fn split(p: &mut Project, at_us: Us) -> Result<()> {
    check_time(at_us)?;
    let (i, start) =
        clip_at(&p.timeline.main, at_us).ok_or_else(|| invalid(code!("model.clip.noneToSplit")))?;
    let clip = p.timeline.main[i].clone();
    let end = start.saturating_add(clip.duration_us());
    if at_us.saturating_sub(start) < MIN_DURATION_US || end.saturating_sub(at_us) < MIN_DURATION_US
    {
        return Err(invalid(code!("model.clip.splitEdge")));
    }
    let cut = clip
        .source_in_us
        .saturating_add(unscaled(at_us.saturating_sub(start), clip.speed));
    let mut ids = IdPool::of(p);
    let mut first = clip.clone();
    first.source_out_us = cut;
    first.fade_out_us = 0;
    first.transition_out = None;
    let mut second = clip;
    second.id = ids.derive(&first.id);
    second.source_in_us = cut;
    second.fade_in_us = 0;
    clamp_fades(&mut first);
    clamp_fades(&mut second);
    // Content from the cut on (including the old end) now plays from the second piece.
    for item in p
        .timeline
        .tracks
        .iter_mut()
        .flat_map(|t| t.items.iter_mut())
    {
        if let Some(attach) = item.attach.as_mut() {
            if attach.clip_id == first.id
                && attach.source_us >= cut
                && attach.source_us <= second.source_out_us
            {
                attach.clip_id = second.id.clone();
            }
        }
    }
    split_source_layers(p, &first, &second, start, &mut ids);
    p.timeline.main[i] = first;
    p.timeline.main.insert(i.saturating_add(1), second);
    Ok(())
}

/// Separated source layers (`DetachSource`: the camera's video, a mic's or game sound's audio) that start on the left
/// piece of a split are cut at the split too (as a cut splits video layers), so each part follows its own piece
/// through speed changes, moves and removals: a removed piece takes its voice with it, and a moved one keeps it.
/// `first` (starting at `start`) and `second` are the two pieces. The right part gets an id derived from the layer's
/// and is anchored where `second` starts; a sound's fades stay at its outer ends, as `SplitItem` leaves them. Layers
/// reaching less than 10 ms past either side of the split stay whole.
fn split_source_layers(p: &mut Project, first: &Clip, second: &Clip, start: Us, ids: &mut IdPool) {
    let files = separable_files(p, first);
    if files.is_empty() {
        return;
    }
    let cut_at = start.saturating_add(first.duration_us());
    for track in p.timeline.tracks.iter_mut() {
        let mut parts = Vec::new();
        for item in track.items.iter_mut() {
            let Some(anchor) = item.attach.as_ref().filter(|a| {
                a.clip_id == first.id
                    && first.source_in_us <= a.source_us
                    && a.source_us < first.source_out_us
            }) else {
                continue;
            };
            let at = scaled(
                anchor.source_us.saturating_sub(first.source_in_us),
                first.speed,
            );
            let left = cut_at.saturating_sub(start.saturating_add(at));
            let right = item.duration_us.saturating_sub(left);
            let separated = matches!(&item.content, ItemContent::Video { asset_id, .. }
                | ItemContent::Audio { asset_id, .. } if files.contains(asset_id));
            if !separated || left < MIN_DURATION_US || right < MIN_DURATION_US {
                continue;
            }
            let mut part = item.clone();
            part.id = ids.derive(&item.id);
            part.start_us = cut_at;
            part.duration_us = right;
            if let Some((a, b)) = item
                .layout
                .motion
                .as_ref()
                .map(|m| m.split(left, item.duration_us))
            {
                item.layout.motion = motion::kept(a);
                part.layout.motion = motion::kept(b);
            }
            part.attach = Some(Attach {
                clip_id: second.id.clone(),
                source: second.source.clone(),
                source_us: second.source_in_us,
            });
            advance_source(&mut part.content, left);
            if let ItemContent::Audio { fade_in_us, .. } = &mut part.content {
                *fade_in_us = 0;
            }
            if let ItemContent::Audio { fade_out_us, .. } = &mut item.content {
                *fade_out_us = 0;
            }
            item.duration_us = left;
            fit_fades(item);
            fit_fades(&mut part);
            parts.push(part);
        }
        for part in parts {
            insert_sorted(track, part);
        }
    }
}

/// The files `DetachSource` can make layers of for a recording clip: every run of each source but the main footage.
fn separable_files(p: &Project, clip: &Clip) -> Vec<Id> {
    let ClipSource::Recording { id } = &clip.source else {
        return Vec::new();
    };
    p.recording(id).map_or_else(Vec::new, |r| {
        r.sources
            .iter()
            .filter(|s| s.id != r.primary)
            .flat_map(|s| s.runs.iter().map(|run| run.asset_id.clone()))
            .collect()
    })
}

/// The camera files of a recording clip (`query::camera_files`).
fn camera_assets(p: &Project, clip: &Clip) -> Vec<Id> {
    crate::query::camera_files(p, clip)
}

pub(super) fn delete_ranges(p: &mut Project, ranges: &[RangeUs]) -> Result<()> {
    for r in ranges {
        check_time(r.start)?;
        check_time(r.end)?;
    }
    ripple_delete(p, ranges)
}

pub(super) fn remove_clip(p: &mut Project, id: &str, places: &mut Places) -> Result<()> {
    let i = clip_index(p, id, places)?;
    let (start, end) = clip_span(&p.timeline.main, i);
    ripple_delete(p, &[RangeUs { start, end }])
}

/// Cut `ranges` out of the main track, close the gaps and carry attached layers along. The removed ranges are indexed
/// once (`Ripple`), so each clip and layer finds its cuts by binary search: O((C + L) log R) plus the
/// pieces made, not O((C + L) × R).
fn ripple_delete(p: &mut Project, ranges: &[RangeUs]) -> Result<()> {
    let merged = merge_ranges(ranges, main_track_duration(&p.timeline.main));
    if merged.is_empty() {
        return Err(invalid(code!("model.clip.nothingToRemove")));
    }
    let removed = Ripple::new(&with_slivers(&p.timeline.main, merged));
    let mut ids = IdPool::of(p);
    let starts = ripple_items(p, &removed, &mut ids);
    let origins = cut_main(&mut p.timeline.main, &removed, &mut ids);
    reanchor(p, &starts, &origins);
    Ok(())
}

pub(super) fn trim_clip(
    p: &mut Project,
    id: &str,
    edge: Edge,
    source_us: Us,
    places: &mut Places,
) -> Result<()> {
    check_time(source_us)?;
    let i = clip_index(p, id, places)?;
    let (start, end) = clip_span(&p.timeline.main, i);
    let mut trimmed = p.timeline.main[i].clone();
    let inward = match edge {
        Edge::Start => source_us > trimmed.source_in_us,
        Edge::End => source_us < trimmed.source_out_us,
    };
    set_edge(&mut trimmed, edge, source_us);
    if !inward {
        p.timeline.main[i] = trimmed;
        return Ok(());
    }
    if trimmed.duration_us() < MIN_DURATION_US {
        return Err(invalid(code!("model.clip.minLength")));
    }
    // Ripple by exactly what the clip loses on the timeline.
    let cut = end
        .saturating_sub(start)
        .saturating_sub(trimmed.duration_us())
        .max(0);
    let (removed, kept) = match edge {
        Edge::Start => {
            let at = start.saturating_add(cut);
            (RangeUs { start, end: at }, RangeUs { start: at, end })
        }
        Edge::End => {
            let at = end.saturating_sub(cut);
            (RangeUs { start: at, end }, RangeUs { start, end: at })
        }
    };
    let removed = Ripple::new(&merge_ranges(&[removed], end));
    let mut origins = spans(&p.timeline.main);
    if let Some(origin) = origins.get_mut(i) {
        origin.1 = kept;
    }
    let mut ids = IdPool::of(p);
    let starts = ripple_items(p, &removed, &mut ids);
    p.timeline.main[i] = trimmed;
    reanchor(p, &starts, &origins);
    Ok(())
}

/// Each clip's own index and timeline span.
fn spans(main: &[Clip]) -> Origins {
    let mut start: Us = 0;
    main.iter()
        .enumerate()
        .map(|(i, clip)| {
            let end = start.saturating_add(clip.duration_us());
            let span = RangeUs { start, end };
            start = end;
            (i, span)
        })
        .collect()
}

fn set_edge(clip: &mut Clip, edge: Edge, source_us: Us) {
    match edge {
        Edge::Start => clip.source_in_us = source_us,
        Edge::End => clip.source_out_us = source_us,
    }
    clamp_fades(clip);
}

pub(super) fn update_clip(
    p: &mut Project,
    id: &str,
    patch: &ClipPatch,
    places: &mut Places,
) -> Result<()> {
    let i = clip_index(p, id, places)?;
    let clip = &p.timeline.main[i];
    if patch.speed.is_some_and(|s| s != clip.speed) {
        let start = places.start(&p.timeline.main, i);
        if sound_separated(p, clip, start, start.saturating_add(clip.duration_us())) {
            // Named by where it plays: in a batch ("speed every clip up"), one such clip refuses the lot.
            return Err(invalid_with(msg!(
                "model.source.speedSeparated",
                atUs = start
            )));
        }
    }
    update_clip_at(p, i, patch, None);
    Ok(())
}

/// Whether a sound layer of one of `clip`'s recording's sound sources plays over `[start, end)` (its timeline span):
/// its sound was separated there (`DetachSource`). Sound layers have no speed, so the clip's speed can't change under
/// them without the two drifting apart (sync must hold): `update_clip` refuses it. O(layers × the recording's sound
/// files), no allocation.
pub(crate) fn sound_separated(p: &Project, clip: &Clip, start: Us, end: Us) -> bool {
    let ClipSource::Recording { id } = &clip.source else {
        return false;
    };
    let Some(r) = p.recording(id) else {
        return false;
    };
    let recorded = |file: &str| {
        r.audio_sources()
            .any(|s| s.runs.iter().any(|run| run.asset_id == file))
    };
    p.timeline.tracks.iter().flat_map(|t| &t.items).any(|i| {
        i.start_us < end
            && i.end_us() > start
            && matches!(&i.content, ItemContent::Audio { asset_id, .. } if recorded(asset_id))
    })
}

/// [`update_clip`] on clip `i`. A new speed re-times the clip's separated camera layers: the ones at `cameras` when the
/// caller already found them (a run of speed changes, `speed_run`), else every layer is looked at.
pub(super) fn update_clip_at(
    p: &mut Project,
    i: usize,
    patch: &ClipPatch,
    cameras: Option<&[(usize, usize)]>,
) {
    let clip = &mut p.timeline.main[i];
    if let Some(v) = patch.speed {
        clip.speed = v;
    }
    if let Some(v) = patch.volume {
        clip.volume = v;
    }
    if let Some(v) = patch.muted {
        clip.muted = v;
    }
    if let Some(v) = patch.fade_in_us {
        clip.fade_in_us = v;
    }
    if let Some(v) = patch.fade_out_us {
        clip.fade_out_us = v;
    }
    if let Some(v) = &patch.look {
        clip.look = v.clone();
    }
    if let Some(v) = &patch.camera {
        clip.camera = Some(v.clone());
    }
    if let Some(v) = &patch.framing {
        clip.framing = v.clone();
    }
    if let Some(v) = &patch.effects {
        clip.effects = v.clone();
    }
    if patch.speed.is_some() {
        // The new speed changes the clip's length; fades the patch didn't set shrink to fit
        // (fades it did set must fit as given, or validation refuses the edit).
        let d = clip.duration_us();
        let room = |other: Us| d.saturating_sub(other).max(0);
        match (patch.fade_in_us, patch.fade_out_us) {
            (None, None) => clamp_fades(clip),
            (Some(_), None) => clip.fade_out_us = clip.fade_out_us.clamp(0, room(clip.fade_in_us)),
            (None, Some(_)) => clip.fade_in_us = clip.fade_in_us.clamp(0, room(clip.fade_out_us)),
            (Some(_), Some(_)) => {}
        }
    }
    if let Some(speed) = patch.speed {
        match cameras {
            Some([]) => {}
            Some(at) => {
                let camera = camera_assets(p, &p.timeline.main[i]);
                for &(t, k) in at {
                    camera_layer_follows(&mut p.timeline.tracks[t].items[k], &camera, speed);
                }
            }
            None => camera_layers_follow_speed(p, i, speed),
        }
    }
}

const NO_ROOM: &str = code!("model.transition.noRoom");

pub(super) fn set_transition(
    p: &mut Project,
    clip_id: &str,
    t: Option<&Transition>,
    places: &mut Places,
) -> Result<()> {
    let i = clip_index(p, clip_id, places)?;
    let Some(t) = t else {
        p.timeline.main[i].transition_out = None;
        return Ok(());
    };
    super::check_length(t.duration_us)?;
    if i + 1 >= p.timeline.main.len() {
        return Err(invalid(code!("model.transition.noNextClip")));
    }
    if t.duration_us < MIN_TRANSITION_US {
        return Err(invalid(code!("model.transition.tooShort")));
    }
    let max = max_transition_us(p, i);
    if max < MIN_TRANSITION_US {
        return Err(invalid(NO_ROOM));
    }
    if t.duration_us > max {
        // Floored to tenths, so the length named always fits (1.96 s reads 1.9 s, not 2.0 s).
        return Err(invalid_with(msg!(
            "model.transition.max",
            maxDurationUs = max / 100_000 * 100_000
        )));
    }
    p.timeline.main[i].transition_out = Some(t.clone());
    Ok(())
}

/// Camera layers separated from `main[i]` (video layers of its recording's camera attached to it)
/// play at the clip's new speed over the same camera footage, so they stay in sync.
fn camera_layers_follow_speed(p: &mut Project, i: usize, speed: f64) {
    let clip = &p.timeline.main[i];
    let camera = camera_assets(p, clip);
    if camera.is_empty() {
        return;
    }
    let clip_id = clip.id.clone();
    for item in p
        .timeline
        .tracks
        .iter_mut()
        .flat_map(|t| t.items.iter_mut())
    {
        if item.attach.as_ref().is_some_and(|a| a.clip_id == clip_id) {
            camera_layer_follows(item, &camera, speed);
        }
    }
}

/// One layer attached to a clip whose speed became `speed`: a video of one of the clip's `camera` files at another speed
/// plays at the new one over the same footage.
pub(super) fn camera_layer_follows(item: &mut Item, camera: &[Id], speed: f64) {
    if let ItemContent::Video {
        asset_id,
        speed: layer_speed,
        ..
    } = &mut item.content
    {
        if camera.contains(asset_id) && *layer_speed != speed {
            let (footage, old) = (unscaled(item.duration_us, *layer_speed), *layer_speed);
            *layer_speed = speed;
            item.duration_us = scaled(footage, speed);
            // Its keys stay on the same footage, as the bubble's do in source time.
            let len = item.duration_us;
            item.layout.motion = item.layout.motion.take().and_then(|mut m| {
                m.rescale(old, speed, len);
                motion::kept(m)
            });
        }
    }
}

/// Kept parts of the main track shorter than 10 ms are dropped by the cut, so they join the
/// removed ranges: layers then ripple by exactly what leaves the main track. `merged` comes from
/// `merge_ranges` over the track's length. Clips run in order, so each one's scan starts at the
/// first range ending after its start, searched from where the last one's began.
fn with_slivers(main: &[Clip], merged: Vec<RangeUs>) -> Vec<RangeUs> {
    let mut slivers = Vec::new();
    let (mut start, mut first): (Us, usize) = (0, 0);
    for clip in main {
        let end = start.saturating_add(clip.duration_us());
        first += merged[first..].partition_point(|r| r.end <= start);
        slivers.extend(
            kept_parts(start, end, &merged[first..])
                .into_iter()
                .filter(|r| r.len() < MIN_DURATION_US),
        );
        start = end;
    }
    if slivers.is_empty() {
        return merged;
    }
    slivers.extend(merged);
    merge_ranges(&slivers, start)
}

/// Cut `removed` (sorted, merged timeline ranges) out of the main track. A clip's first kept
/// piece keeps its id; later pieces get ids derived from it.
fn cut_main(main: &mut Vec<Clip>, removed: &Ripple, ids: &mut IdPool) -> Origins {
    // Each range splits at most one clip in two.
    let pieces = main.len().saturating_add(removed.ranges().len());
    let mut out = Vec::with_capacity(pieces);
    let mut origins = Vec::with_capacity(pieces);
    let mut start: Us = 0;
    for (from, clip) in main.drain(..).enumerate() {
        let end = start.saturating_add(clip.duration_us());
        let parts: Vec<RangeUs> = removed
            .kept(start, end)
            .into_iter()
            .filter(|r| r.len() >= MIN_DURATION_US)
            .collect();
        let last = parts.len().saturating_sub(1);
        ids.reserve(last);
        let source_at = |t: Us| {
            clip.source_in_us
                .saturating_add(unscaled(t.saturating_sub(start), clip.speed))
        };
        for (n, part) in parts.iter().enumerate() {
            let mut piece = clip.clone();
            if n > 0 {
                piece.id = ids.derive(&clip.id);
            }
            piece.source_in_us = source_at(part.start);
            piece.source_out_us = if part.end == end {
                clip.source_out_us
            } else {
                source_at(part.end)
            };
            piece.fade_in_us = if n == 0 && part.start == start {
                clip.fade_in_us
            } else {
                0
            };
            piece.fade_out_us = if n == last && part.end == end {
                clip.fade_out_us
            } else {
                0
            };
            // The transition stays with the last piece even when the clip's end was cut away (as trimming the end
            // keeps it); `transitions::normalize` refits it or drops it.
            piece.transition_out = if n == last {
                clip.transition_out.clone()
            } else {
                None
            };
            clamp_fades(&mut piece);
            out.push(piece);
            origins.push((from, *part));
        }
        start = end;
    }
    *main = out;
    origins
}

/// Ripple attached layers through the removal of `removed` (sorted, merged; main track not yet
/// cut). Returns where each layer (piece) of the tracks, in order, now starts (`None`: a layer
/// that doesn't follow the main track), for [`reanchor`] after the cut.
fn ripple_items(p: &mut Project, removed: &Ripple, ids: &mut IdPool) -> Vec<Option<LayerStart>> {
    let layers: usize = p.timeline.tracks.iter().map(|t| t.items.len()).sum();
    let mut starts = Vec::with_capacity(layers);
    let main = MainIndex::new(&p.timeline.main);
    for track in p.timeline.tracks.iter_mut() {
        let mut next = Vec::with_capacity(track.items.len());
        for mut item in std::mem::take(&mut track.items) {
            // Detached layers never move; anchors that no longer resolve are left for
            // `normalize` to detach.
            let located = item
                .attach
                .clone()
                .and_then(|a| main.locate(&a).map(|(clip, at)| (clip, at, a)));
            let Some((clip, at, attach)) = located else {
                next.push(item);
                starts.push(None);
                continue;
            };
            // Start from where the anchor puts the layer (stored starts can be stale).
            item.start_us = at;
            let pieces = ripple_item(item, removed, ids);
            // A long video layer can be cut into thousands of pieces: room for them at once.
            next.reserve(pieces.len());
            starts.reserve(pieces.len());
            for (piece, from) in pieces {
                let anchor = (from == at).then(|| (clip, attach.clone()));
                starts.push(Some(LayerStart { at: from, anchor }));
                next.push(piece);
            }
        }
        track.items = next;
    }
    starts
}

/// Static layers (text, image, zoom) map their start and end through the ripple; video and audio
/// layers keep only the footage that stays, split into pieces. Pieces under 10 ms are dropped.
/// Each piece comes with the old timeline moment it now starts at.
fn ripple_item(item: Item, removed: &Ripple, ids: &mut IdPool) -> Vec<(Item, Us)> {
    let (s, e) = (item.start_us, item.end_us());
    let parts: Vec<RangeUs> = removed
        .kept(s, e)
        .into_iter()
        .filter(|r| r.len() >= MIN_DURATION_US)
        .collect();
    if parts.is_empty() {
        return Vec::new();
    }
    if !matches!(
        item.content,
        ItemContent::Video { .. } | ItemContent::Audio { .. }
    ) {
        let mut piece = item;
        piece.start_us = removed.map(s);
        piece.duration_us = removed.map(e).saturating_sub(piece.start_us);
        let len = piece.duration_us;
        generator_pieces::cut(&mut piece.content, e.saturating_sub(s).saturating_sub(len));
        piece.layout.motion = piece.layout.motion.take().and_then(|mut m| {
            ripple_motion(&mut m, (s, e), removed, len);
            motion::kept(m)
        });
        return if piece.duration_us < MIN_DURATION_US {
            Vec::new()
        } else {
            vec![(piece, removed.first_kept(s))]
        };
    }
    let last = parts.len().saturating_sub(1);
    ids.reserve(last);
    let mut pieces = Vec::with_capacity(parts.len());
    for (n, part) in parts.iter().enumerate() {
        let mut piece = item.clone();
        if n > 0 {
            piece.id = ids.derive(&item.id);
        }
        piece.start_us = removed.map(part.start);
        piece.duration_us = part.len();
        let offset = part.start.saturating_sub(s);
        // Each piece keeps the motion of its stretch; the enter stays on the first and the exit on the last.
        piece.layout.motion = item.layout.motion.as_ref().and_then(|m| {
            motion::kept(m.piece(
                offset,
                part.len(),
                n == 0 && part.start == s,
                n == last && part.end == e,
            ))
        });
        match &mut piece.content {
            ItemContent::Video {
                source_in_us,
                speed,
                ..
            } => *source_in_us = source_in_us.saturating_add(unscaled(offset, *speed)),
            ItemContent::Audio {
                source_in_us,
                fade_in_us,
                fade_out_us,
                drift_ppm,
                ..
            } => {
                *source_in_us = source_in_us.saturating_add(drifted(offset, *drift_ppm));
                if !(n == 0 && part.start == s) {
                    *fade_in_us = 0;
                }
                if !(n == last && part.end == e) {
                    *fade_out_us = 0;
                }
                let len = part.len();
                *fade_in_us = (*fade_in_us).clamp(0, len);
                *fade_out_us = (*fade_out_us).clamp(0, len.saturating_sub(*fade_in_us));
            }
            _ => {}
        }
        pieces.push((piece, part.start));
    }
    pieces
}

/// A static layer's motion through a ripple of the layer at `[s, e)`, now `len` long (`Motion::cut`): keys keep their
/// place on what stays and go with what is cut, and values hold at the joins. The cuts inside the layer are found by
/// binary search in `removed`.
fn ripple_motion(m: &mut Motion, (s, e): (Us, Us), removed: &Ripple, len: Us) {
    let removed = removed.ranges();
    let lo = removed.partition_point(|r| r.end <= s);
    let inside = &removed[lo..lo + removed[lo..].partition_point(|r| r.start < e)];
    let cuts: Vec<(Us, Us)> = inside
        .iter()
        .map(|r| (r.start.max(s) - s, r.end.min(e) - s))
        .collect();
    m.cut(&cuts, e - s, len);
}

/// Re-anchor rippled layers on the rebuilt main track (`normalize` then derives their starts).
/// `starts` runs parallel to the tracks' layers as [`ripple_items`] left them: nothing between
/// the two reorders a track.
fn reanchor(p: &mut Project, starts: &[Option<LayerStart>], origins: &[(usize, RangeUs)]) {
    debug_assert_eq!(
        p.timeline
            .tracks
            .iter()
            .map(|t| t.items.len())
            .sum::<usize>(),
        starts.len()
    );
    let main = &p.timeline.main;
    let items = p
        .timeline
        .tracks
        .iter_mut()
        .flat_map(|t| t.items.iter_mut());
    for (item, start) in items.zip(starts) {
        if let Some(start) = start {
            item.attach = anchor_after_cut(main, origins, start);
        }
    }
}

/// A rippled layer's anchor on the rebuilt main track, worked out in source time. `origins`
/// runs parallel to `main`, in old-track order: pieces of one old clip are adjacent and the old
/// spans are disjoint and ascending, so every lookup is a binary search (a clip cut into
/// thousands of pieces is never scanned per layer).
fn anchor_after_cut(
    main: &[Clip],
    origins: &[(usize, RangeUs)],
    start: &LayerStart,
) -> Option<Attach> {
    // Its own moment survived: keep it, on the piece of the same old clip that now plays it
    // (a piece that starts there wins over one that ends there).
    if let Some((from, attach)) = &start.anchor {
        let su = attach.source_us;
        let first = origins.partition_point(|(o, _)| o < from);
        let end = first.saturating_add(origins[first..].partition_point(|(o, _)| o <= from));
        let own = main.get(first..end).unwrap_or(&[]);
        // One clip's pieces play disjoint, ascending stretches of its source.
        let after = own.partition_point(|c| c.source_out_us <= su);
        let piece = own.get(after).filter(|c| c.source_in_us <= su).or_else(|| {
            after
                .checked_sub(1)
                .and_then(|i| own.get(i))
                .filter(|c| c.source_out_us == su)
        });
        if let Some(c) = piece {
            return Some(Attach {
                clip_id: c.id.clone(),
                ..attach.clone()
            });
        }
    }
    // Otherwise it starts at the kept moment `at`: anchor to the piece whose content was there,
    // measured from that piece's own start, so a layer at the start of a kept stretch lands
    // exactly on the piece's first frame.
    let k = origins.partition_point(|(_, old)| old.end <= start.at);
    let found = origins
        .get(k)
        .filter(|(_, old)| old.start <= start.at)
        .zip(main.get(k));
    match found {
        Some(((_, old), c)) => {
            let offset = unscaled(start.at.saturating_sub(old.start), c.speed);
            Some(Attach {
                clip_id: c.id.clone(),
                source: c.source.clone(),
                source_us: c
                    .source_in_us
                    .saturating_add(offset)
                    .min(c.source_out_us)
                    .max(c.source_in_us),
            })
        }
        // Nothing kept after it: it sits at the end of the timeline.
        None => anchor_at(main, main_track_duration(main)),
    }
}
