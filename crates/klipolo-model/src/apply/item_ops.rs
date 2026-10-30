//! Edits to layers on overlay, audio and zoom tracks.
//!
//! Op-supplied times are checked against `0..=MAX_TIME_US` before use and all other arithmetic
//! saturates. New tracks, split parts and camera layers get ids derived from what they came from,
//! so repeating an edit repeats its ids.

use crate::error::{invalid, invalid_with, not_found, Result};
use crate::ids::Id;
use crate::layout::{Anchor, BoxN, Layout};
use crate::messages::{code, msg};
use crate::motion;
use crate::ops::{Edge, ItemPatch, TrackTarget};
use crate::project::Project;
use crate::query::{anchor_at, fits, locate_attach};
use crate::time::{drifted, scaled, unscaled, Us, MIN_DURATION_US};
use crate::timeline::{Attach, Clip, Item, ItemContent, Track, TrackKind};

use super::generator_pieces;
use super::places::Places;
use super::{
    check_length, check_time, default_track_name, derived_id, fits_in_order, fresh_id, id_taken,
    insert_sorted, place_above, split_tracks, LOCKED,
};

const WRONG_TRACK: &str = code!("model.layer.wrongTrack");
const TOO_SHORT: &str = code!("model.layer.minLength");

/// (track, index) of a layer on an unlocked track.
fn locate(p: &Project, id: &str, places: &mut Places) -> Result<(usize, usize)> {
    let (t, i) = places
        .item(&p.timeline.tracks, id)
        .ok_or_else(|| not_found("layer", id))?;
    if p.timeline.tracks[t].locked {
        return Err(invalid(LOCKED));
    }
    Ok((t, i))
}

fn take(p: &mut Project, id: &str, places: &mut Places) -> Result<(usize, Item)> {
    let (t, i) = locate(p, id, places)?;
    Ok((t, p.timeline.tracks[t].items.remove(i)))
}

/// Index of track `id`, which must hold `kind` layers and be unlocked.
fn target_track(p: &Project, id: &str, kind: TrackKind) -> Result<usize> {
    let t = p
        .timeline
        .tracks
        .iter()
        .position(|t| t.id == id)
        .ok_or_else(|| not_found("track", id))?;
    let track = &p.timeline.tracks[t];
    if track.kind != kind {
        return Err(invalid(WRONG_TRACK));
    }
    if track.locked {
        return Err(invalid(LOCKED));
    }
    Ok(t)
}

/// Anchor at timeline position `at` when `attached` (the user chose that position).
fn attach_at(p: &Project, at: Us, attached: bool) -> Option<Attach> {
    if attached {
        anchor_at(&p.timeline.main, at)
    } else {
        None
    }
}

/// `attach` moved by `offset` timeline µs, worked out in source time from the clip that plays it,
/// so rounding at non-unit speeds can't hand the layer to a neighbouring clip. Crossing a clip's
/// end lands on the next clip (a clip that starts at a moment wins over one that ends there).
/// Falls back to `anchor_at(at)` when the anchor no longer resolves or `offset` reaches back
/// before its clip.
pub(super) fn shifted_anchor(main: &[Clip], attach: &Attach, offset: Us, at: Us) -> Option<Attach> {
    let on = |c: &Clip, source_us: Us| Attach {
        clip_id: c.id.clone(),
        source: c.source.clone(),
        source_us,
    };
    let Some((k, _)) = locate_attach(main, attach) else {
        return anchor_at(main, at);
    };
    if offset < 0 {
        let clip = &main[k];
        let su = attach
            .source_us
            .saturating_sub(unscaled(offset.saturating_neg(), clip.speed));
        return if su >= clip.source_in_us {
            Some(on(clip, su))
        } else {
            anchor_at(main, at)
        };
    }
    let (mut su, mut left) = (attach.source_us, offset);
    for (j, c) in main.iter().enumerate().skip(k) {
        if j > k {
            su = c.source_in_us;
        }
        let room = scaled(c.source_out_us.saturating_sub(su).max(0), c.speed);
        if left < room {
            let at_source = su.saturating_add(unscaled(left, c.speed));
            return Some(on(c, at_source.min(c.source_out_us)));
        }
        left = left.saturating_sub(room);
    }
    // The very end of the timeline anchors to the last clip's end; past it nothing does.
    if left == 0 {
        main.last().map(|c| on(c, c.source_out_us))
    } else {
        None
    }
}

/// Whether `[start, end)` fits on `track` (`query::fits`): in O(log layers) once the batch is settled, when every track
/// is in order.
fn fits_on(track: &Track, start: Us, end: Us, ignore: Option<&str>, settled: bool) -> bool {
    if settled {
        fits_in_order(track, start, end, ignore)
    } else {
        fits(track, start, end, ignore)
    }
}

/// Put `item` on track `t` if it fits, else on the next free track above. Returns where it went, and tells `places`.
fn place(p: &mut Project, t: usize, item: Item, places: &mut Places) -> (usize, usize) {
    let settled = places.settled;
    let here = fits_on(
        &p.timeline.tracks[t],
        item.start_us,
        item.end_us(),
        Some(&item.id),
        settled,
    );
    let at = if here {
        (t, insert_sorted(&mut p.timeline.tracks[t], item))
    } else {
        let (outside, tracks) = split_tracks(p);
        place_above(tracks, t, item, &|id| outside.contains(id), settled)
    };
    let tracks = &p.timeline.tracks;
    places.put_item(tracks, &tracks[at.0].items[at.1].id, at);
    at
}

/// The lowest unlocked, visible track of the item's kind above every track of that kind with a
/// layer it overlaps in time (so nothing already there hides it), else a new track on top whose id
/// is derived from the item's (`{item}-track`, numbered if taken).
fn auto_track(p: &mut Project, item: &Item, settled: bool) -> usize {
    let kind = item.content.track_kind();
    let (start, end) = (item.start_us, item.end_us());
    let tracks = &p.timeline.tracks;
    let above = tracks
        .iter()
        .rposition(|t| t.kind == kind && !fits_on(t, start, end, None, settled))
        .map_or(0, |t| t.saturating_add(1));
    let usable = |t: &Track| t.kind == kind && !t.locked && !t.hidden;
    if let Some(t) = (above..tracks.len()).find(|&t| usable(&tracks[t])) {
        return t;
    }
    let mut track = Track::new(kind, default_track_name(kind, &p.timeline.tracks));
    track.id = fresh_id(format!("{}-track", item.id), |id| id_taken(p, id));
    p.timeline.tracks.push(track);
    p.timeline.tracks.len().saturating_sub(1)
}

pub(super) fn add(
    p: &mut Project,
    target: &TrackTarget,
    item: Item,
    places: &mut Places,
) -> Result<(usize, usize)> {
    let t = match target {
        TrackTarget::Auto => auto_track(p, &item, places.settled),
        TrackTarget::Track { id } => target_track(p, id, item.content.track_kind())?,
    };
    Ok(place(p, t, item, places))
}

pub(super) fn add_item(
    p: &mut Project,
    target: &TrackTarget,
    item: &Item,
    attach: bool,
    places: &mut Places,
) -> Result<()> {
    check_time(item.start_us)?;
    check_length(item.duration_us)?;
    // An id already in the project is refused here, as validation would refuse the batch: a later op in the batch
    // could otherwise act on either copy.
    if taken(p, &item.id, places) {
        return Err(invalid_with(msg!(
            "model.project.duplicateId",
            id = item.id.as_str()
        )));
    }
    let mut item = item.clone();
    item.attach = attach_at(p, item.start_us, attach);
    // A layer added this way (a paste, a duplicate) is a copy placed by hand, never a source's own.
    item.content.set_tie(None);
    add(p, target, item, places).map(|_| ())
}

/// Audio fades shrink to fit the layer (the fade-in keeps priority).
pub(super) fn fit_fades(item: &mut Item) {
    let d = item.duration_us.max(0);
    if let ItemContent::Audio {
        fade_in_us,
        fade_out_us,
        ..
    } = &mut item.content
    {
        *fade_in_us = (*fade_in_us).clamp(0, d);
        *fade_out_us = (*fade_out_us).clamp(0, d.saturating_sub(*fade_in_us));
    }
}

/// An item with nothing in it, standing in a track's slot while the layer there is updated, so the track doesn't shift
/// (a `Vec::remove` and an insert move every later layer, 816 bytes each). Allocates nothing.
fn placeholder() -> Item {
    Item {
        id: String::new(),
        start_us: 0,
        duration_us: 0,
        attach: None,
        layout: Layout::new(
            Anchor::Center,
            BoxN {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
        ),
        content: ItemContent::Adjustment,
        effects: Vec::new(),
    }
}

/// Returns whether the layer kept its slot. Taking a layer out and placing it back puts it where it was when its length
/// is the same and the batch is settled (its track is in order, and no neighbour starts with it, so it fits there and
/// sorts there), so then it is updated in its slot: the same result in O(1) instead of two shifts of the track.
pub(super) fn update_item(
    p: &mut Project,
    id: &str,
    patch: &ItemPatch,
    places: &mut Places,
) -> Result<bool> {
    if let Some(d) = patch.duration_us {
        check_length(d)?;
    }
    let (t, i) = locate(p, id, places)?;
    let mut item = std::mem::replace(&mut p.timeline.tracks[t].items[i], placeholder());
    let old_speed = video_speed(&item.content);
    let old_duration = item.duration_us;
    let mut speeds = None;
    if let Some(content) = &patch.content {
        if content.track_kind() != item.content.track_kind() {
            return Err(invalid(code!("model.layer.kindChange")));
        }
        let mut content = content.clone();
        // Another file, or its file time slipped by hand, frees it from its source; a patch that ties it anew (Sync
        // lining it up) keeps the new tie; a patch that leaves the file time alone keeps the old one even when it
        // doesn't carry it.
        let slipped = content.played() != item.content.played();
        if slipped && content.tie().is_some() && content.tie() == item.content.tie() {
            content.set_tie(None);
        } else if !slipped && content.tie().is_none() {
            content.set_tie(item.content.tie().cloned());
        }
        item.content = content;
    }
    if let Some(layout) = &patch.layout {
        item.layout = layout.clone();
    }
    if let Some(effects) = &patch.effects {
        item.effects = effects.clone();
    }
    if let Some(d) = patch.duration_us {
        item.duration_us = d;
        if patch.content.is_none() {
            // Fades the patch didn't set shrink to fit the new length, and a split piece's exit moves with its end.
            fit_fades(&mut item);
            generator_pieces::end_moved(&mut item.content, d.saturating_sub(old_duration));
        }
    } else if let Some(old) = old_speed {
        speeds = follow_speed(p, &mut item, old);
    }
    // The motion follows: a new speed rescales its keys onto the same footage, and a new length fits it. Both apply to a
    // layout the patch brings too: the inspector sends the layer's layout with its keys still on the old speed, and a
    // patch with a copied layout and a new length isn't refused.
    match speeds {
        Some((old, new)) => {
            let len = item.duration_us;
            item.layout.motion = item.layout.motion.take().and_then(|mut m| {
                m.rescale(old, new, len);
                motion::kept(m)
            });
        }
        _ if item.duration_us != old_duration => fit_motion(&mut item),
        _ => {}
    }
    let items = &p.timeline.tracks[t].items;
    let s = item.start_us;
    let same_slot = places.settled
        && item.duration_us == old_duration
        && (i == 0 || items[i - 1].start_us < s)
        && items.get(i + 1).is_none_or(|next| next.start_us > s);
    if same_slot {
        p.timeline.tracks[t].items[i] = item;
        return Ok(true);
    }
    p.timeline.tracks[t].items.remove(i);
    place(p, t, item, places);
    Ok(false)
}

/// The layer's motion fits its length (keys past the end go, presets shorten).
pub(super) fn fit_motion(item: &mut Item) {
    let len = item.duration_us;
    item.layout.motion = item.layout.motion.take().and_then(|mut m| {
        m.fit(len);
        motion::kept(m)
    });
}

fn video_speed(content: &ItemContent) -> Option<f64> {
    match content {
        ItemContent::Video { speed, .. } => Some(*speed),
        _ => None,
    }
}

/// A video layer whose speed changed from `old` (to a valid speed) plays the same footage: its
/// length scales, up to the end of the file and at least 10 ms. Returns the old and new speeds when it did.
fn follow_speed(p: &Project, item: &mut Item, old: f64) -> Option<(f64, f64)> {
    let ItemContent::Video {
        asset_id,
        source_in_us,
        speed,
        ..
    } = &item.content
    else {
        return None;
    };
    if *speed == old || !(0.25..=4.0).contains(speed) {
        return None;
    }
    let mut duration = scaled(unscaled(item.duration_us, old), *speed);
    if let Some(asset) = p.asset(asset_id) {
        let left = asset.duration_us.saturating_sub(*source_in_us).max(0);
        duration = duration.min(scaled(left, *speed));
    }
    let new = *speed;
    item.duration_us = duration.max(MIN_DURATION_US);
    Some((old, new))
}

pub(super) fn move_item(
    p: &mut Project,
    id: &str,
    track_id: Option<&str>,
    start_us: Us,
    places: &mut Places,
) -> Result<()> {
    check_time(start_us)?;
    let (from, mut item) = take(p, id, places)?;
    let t = match track_id {
        None => from,
        Some(track_id) => target_track(p, track_id, item.content.track_kind())?,
    };
    item.start_us = start_us;
    item.attach = attach_at(p, start_us, item.attach.is_some());
    place(p, t, item, places);
    Ok(())
}

/// Skip `offset` timeline µs of a video or audio layer's footage.
pub(super) fn advance_source(content: &mut ItemContent, offset: Us) {
    match content {
        ItemContent::Video {
            source_in_us,
            speed,
            ..
        } => *source_in_us = source_in_us.saturating_add(unscaled(offset, *speed)),
        ItemContent::Audio {
            source_in_us,
            drift_ppm,
            ..
        } => *source_in_us = source_in_us.saturating_add(drifted(offset, *drift_ppm)),
        _ => {}
    }
}

pub(super) fn trim_item(
    p: &mut Project,
    id: &str,
    edge: Edge,
    at_us: Us,
    places: &mut Places,
) -> Result<()> {
    check_time(at_us)?;
    let (t, mut item) = take(p, id, places)?;
    let (start, end) = (item.start_us, item.end_us());
    match edge {
        Edge::Start => {
            if end.saturating_sub(at_us) < MIN_DURATION_US {
                return Err(invalid(TOO_SHORT));
            }
            let offset = at_us.saturating_sub(start);
            advance_source(&mut item.content, offset);
            generator_pieces::start_moved(&mut item.content, offset);
            item.attach = item
                .attach
                .take()
                .and_then(|a| shifted_anchor(&p.timeline.main, &a, offset, at_us));
            item.start_us = at_us;
            item.duration_us = end.saturating_sub(at_us);
            // Keys keep their place on the timeline; those before the new start collapse into one.
            let len = item.duration_us;
            item.layout.motion = item.layout.motion.take().and_then(|mut m| {
                m.trim_start(offset, len);
                motion::kept(m)
            });
        }
        Edge::End => {
            if at_us.saturating_sub(start) < MIN_DURATION_US {
                return Err(invalid(TOO_SHORT));
            }
            item.duration_us = at_us.saturating_sub(start);
            generator_pieces::end_moved(&mut item.content, at_us.saturating_sub(end));
            fit_motion(&mut item);
        }
    }
    fit_fades(&mut item);
    place(p, t, item, places);
    Ok(())
}

/// Returns the second part's id.
pub(super) fn split_item(p: &mut Project, id: &str, at_us: Us, places: &mut Places) -> Result<Id> {
    check_time(at_us)?;
    let (t, i) = locate(p, id, places)?;
    let first = &p.timeline.tracks[t].items[i];
    let (start, end) = (first.start_us, first.end_us());
    if !(start..end).contains(&at_us) {
        return Err(invalid(code!("model.layer.notUnderPlayhead")));
    }
    if at_us.saturating_sub(start) < MIN_DURATION_US || end.saturating_sub(at_us) < MIN_DURATION_US
    {
        return Err(invalid(code!("model.layer.splitEdge")));
    }
    let offset = at_us.saturating_sub(start);
    let mut second = first.clone();
    second.id = derived_id(&first.id, |id| taken(p, id, places));
    second.start_us = at_us;
    second.duration_us = end.saturating_sub(at_us);
    advance_source(&mut second.content, offset);
    let whole = end.saturating_sub(start);
    generator_pieces::piece(&mut second.content, offset, whole, &first.id);
    // Each piece keeps the keys on its side plus one at the cut; the enter stays first, the exit goes second.
    let pieces = first
        .layout
        .motion
        .as_ref()
        .map(|m| m.split(offset, end.saturating_sub(start)));
    second.attach = first
        .attach
        .as_ref()
        .and_then(|a| shifted_anchor(&p.timeline.main, a, offset, at_us));
    // Audio fades stay at the outer ends (motion presets did, in `Motion::split`).
    if let ItemContent::Audio { fade_in_us, .. } = &mut second.content {
        *fade_in_us = 0;
    }
    fit_fades(&mut second);
    let (first_motion, second_motion) =
        pieces.map_or((None, None), |(a, b)| (motion::kept(a), motion::kept(b)));
    second.layout.motion = second_motion;
    let first = &mut p.timeline.tracks[t].items[i];
    first.duration_us = offset;
    first.layout.motion = first_motion;
    generator_pieces::piece(&mut first.content, 0, whole, &first.id);
    if let ItemContent::Audio { fade_out_us, .. } = &mut first.content {
        *fade_out_us = 0;
    }
    fit_fades(first);
    let second_id = second.id.clone();
    let k = insert_sorted(&mut p.timeline.tracks[t], second);
    places.put_item(&p.timeline.tracks, &second_id, (t, k));
    Ok(second_id)
}

pub(super) fn remove_item(p: &mut Project, id: &str, places: &mut Places) -> Result<()> {
    take(p, id, places).map(|_| ())
}

/// Attaching keeps an existing anchor; a detached layer anchors at its start.
pub(super) fn set_attach(
    p: &mut Project,
    id: &str,
    attached: bool,
    places: &mut Places,
) -> Result<()> {
    let (t, i) = locate(p, id, places)?;
    let item = &p.timeline.tracks[t].items[i];
    let attach = match (&item.attach, attached) {
        (_, false) => None,
        (Some(existing), true) => Some(existing.clone()),
        (None, true) => Some(
            anchor_at(&p.timeline.main, item.start_us)
                .ok_or_else(|| invalid(code!("model.layer.attachOutside")))?,
        ),
    };
    p.timeline.tracks[t].items[i].attach = attach;
    Ok(())
}

/// Whether any of the project's ids is `id` (`id_taken`), with the batch's indexes: O(media + tracks) instead of a
/// scan of every clip and layer.
pub(super) fn taken(p: &Project, id: &str, places: &mut Places) -> bool {
    p.id == id
        || p.assets.iter().any(|a| a.id == id)
        || p.recordings.iter().any(|r| r.id == id)
        || p.luts.iter().any(|l| l.id == id)
        || p.timeline.tracks.iter().any(|t| t.id == id)
        || places.has_clip_or_item(p, id)
}
