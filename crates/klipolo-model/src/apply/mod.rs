//! Applying ops. `apply_ops` is the only public entry point and is all-or-nothing.

mod detach;
mod effect_ops;
pub mod generator_pieces;
mod item_ops;
mod main_ops;
mod places;
mod project_ops;
mod recording_ops;
#[cfg(any(test, all(feature = "verify-normalize", debug_assertions)))]
mod reference;
mod slip;
mod speed_lanes;
mod speed_run;
mod sync_ops;

pub(crate) use main_ops::sound_separated;
use places::Places;

use std::collections::{HashMap, HashSet};

use crate::error::{invalid, Result};
use crate::ids::Id;
use crate::messages::code;
use crate::ops::Op;
use crate::project::{Asset, ColorLut, Project, Recording};
use crate::query::{fits, locate_one, MainIndex};
use crate::time::{Us, MAX_TIME_US, OVERLAP_TOLERANCE_US};
use crate::timeline::{Clip, Item, Timeline, Track, TrackKind};
use crate::validate::validate;

/// Batch applications on this thread (feature `count-applies`, tests only): each [`apply_part`], which every
/// [`apply_ops`] goes through, counts one. The app's one-apply test reads it.
#[cfg(feature = "count-applies")]
pub mod count {
    use std::cell::Cell;

    thread_local! {
        static N: Cell<usize> = const { Cell::new(0) };
    }

    pub fn get() -> usize {
        N.with(Cell::get)
    }

    pub(crate) fn bump() {
        N.with(|n| n.set(n.get() + 1));
    }
}

/// Apply `ops` in order to a copy of `project`. Returns the validated result or the first error.
///
/// After each op the project is normalised ([`normalize`]: attached layers where their anchors put them, no
/// overlaps), as the next op must see it. The first op of a batch is followed by the whole-project pass, because the
/// project passed in may come from anywhere. Each later op is settled from what it [`Touched`], with exactly the
/// result the whole-project pass would give, so a 100-op batch costs about one normalise instead of 100. Only an op
/// that changes the main track's timing runs the whole pass again: every anchor may resolve elsewhere; and a run of
/// speed changes runs it once for the run when no state the run passes through would bump a layer (`speed_run`).
///
/// Ops find their clips and layers through one index per batch (`places`), so a batch of n ops on a project with c
/// clips and l layers costs O(n + c + l) for its lookups, not O(n · (c + l)).
///
/// Transitions are refitted once, after the last op: one gesture is one batch, and a state in its middle (two clips
/// moved one at a time) mustn't drop a transition that fits again at its end. Ops never move clips for a transition,
/// so nothing in between needs it fitted; a `SetTransition` in between sees its neighbours not yet refitted, which
/// are never shorter, so at worst it refuses a length the end state would allow.
pub fn apply_ops(project: &Project, ops: &[Op]) -> Result<Project> {
    let mut next = project.clone();
    apply_in_place(&mut next, ops)?;
    Ok(next)
}

/// [`apply_ops`] on `next` itself, without the copy: the same result when it succeeds. On an error `next` is left
/// part-way through the batch, so only a caller that then drops it may use this (the chat compiler's scratch copy).
pub fn apply_in_place(next: &mut Project, ops: &[Op]) -> Result<()> {
    apply_part(next, ops)?;
    finish_parts(next)
}

/// One part of a batch applied in parts (the chat compiler applies each call's ops as it compiles it, so the next
/// call sees them): its ops apply and settle as in [`apply_ops`], but the batch's last step waits for
/// [`finish_parts`]. Until then transitions keep the lengths ops gave them, so a later op is checked against those,
/// as in one `apply_ops`, and nothing is validated. Every part in order, then `finish_parts` once, gives exactly
/// `apply_ops` of all their ops at once: a part's first op runs the whole-project pass, which on a project settled
/// by the op before it gives what settling gives. On an error `next` is left part-way, as with [`apply_in_place`].
pub fn apply_part(next: &mut Project, ops: &[Op]) -> Result<()> {
    #[cfg(feature = "count-applies")]
    count::bump();
    let mut places = Places::default();
    let mut k = 0;
    // A run of speed changes is planned once: the ops it couldn't lay out once go one by one up to its end.
    let mut planned_to = 0;
    while k < ops.len() {
        if places.settled && k >= planned_to {
            let run = speed_run::len(&ops[k..]);
            if run > 1 {
                planned_to = k + run;
                let done = speed_run::apply(next, &ops[k..k + run], &mut places);
                if done > 0 {
                    k += done;
                    continue;
                }
            }
        }
        let op = &ops[k];
        let touched = apply_one(next, op, &mut places)?;
        indexes_after(op, &touched, &mut places);
        // Tests only: the whole pass as it is, and as it was before batches settled incrementally.
        #[cfg(all(feature = "verify-normalize", debug_assertions))]
        let references = {
            let (mut now, mut before) = (next.clone(), next.clone());
            normalize(&mut now);
            reference::normalize_before_pa1(&mut before);
            [
                (now, "the whole-project normalise"),
                (before, "the normalise before batches settled incrementally"),
            ]
        };
        if k == 0 {
            whole(next);
            places.layers_moved();
            places.settled = true;
        } else if is_speed(op) {
            // The clips stay where they were, so their index serves the whole pass.
            whole_indexed(next, &mut places);
        } else {
            settle(next, &touched, &mut places);
        }
        // Tests only: settling gave what both whole passes give. Debug output is the fallback, because a corrupt
        // project's NaN never equals itself.
        #[cfg(all(feature = "verify-normalize", debug_assertions))]
        for (reference, name) in &references {
            assert!(
                *next == *reference || format!("{next:?}") == format!("{reference:?}"),
                "settling after {op:?} differs from {name}"
            );
        }
        #[cfg(all(feature = "verify-normalize", debug_assertions))]
        places.verify(next);
        k += 1;
    }
    Ok(())
}

fn is_speed(op: &Op) -> bool {
    matches!(op, Op::UpdateClip { patch, .. } if patch.speed.is_some())
}

/// What an op leaves stale in the batch's indexes.
fn indexes_after(op: &Op, touched: &Touched, places: &mut Places) {
    if matches!(
        op,
        Op::InsertClip { .. }
            | Op::MoveClip { .. }
            | Op::Split { .. }
            | Op::DeleteRanges { .. }
            | Op::RemoveClip { .. }
    ) {
        places.clips_changed();
    }
    if matches!(touched, Touched::Main) {
        // New clip timing, and a cut or split may have cut layers into pieces with new ids.
        places.timing_changed();
        places.items_changed();
    }
    if !matches!(op, Op::DetachSource { .. }) {
        places.layers_moved();
    }
}

/// A batch's last step (see [`apply_ops`]): transitions refitted once, against the clips as the batch left them, then
/// the whole project validated.
pub fn finish_parts(next: &mut Project) -> Result<()> {
    crate::transitions::normalize(next);
    generator_pieces::sync_scene_requirement(next);
    validate(next)
}

/// What an op changed that [`normalize`] reads. From the end of a batch's first op every track is in order, and an op
/// can only put out of order the tracks of the layers it touched, so settling looks at those tracks only.
enum Touched {
    /// No clip timing, and no layer's anchor, start, length or track.
    Nothing,
    /// The anchors or starts of the layers with these ids (a moved layer, a new one).
    Layers(Vec<Id>),
    /// Layers placed again without new anchors (an update that changed a length): their tracks may be out of order.
    Placed(Vec<Id>),
    /// The main track's clips, their order or their timing: every anchor may resolve elsewhere.
    Main,
}

/// Applies `op` and says what it touched. The match is exhaustive, so a new op has to say too.
fn apply_one(p: &mut Project, op: &Op, places: &mut Places) -> Result<Touched> {
    let nothing = |done: Result<()>| done.map(|()| Touched::Nothing);
    let main = |done: Result<()>| done.map(|()| Touched::Main);
    let layer = |done: Result<()>, id: &Id| done.map(|()| Touched::Layers(vec![id.clone()]));
    match op {
        Op::Rename { name } => nothing(project_ops::rename(p, name)),
        // Layouts only: positions on screen, not in time.
        Op::SetSettings { settings } => nothing(project_ops::set_settings(p, settings)),
        Op::SetCaptions { captions } => nothing(project_ops::set_captions(p, captions)),
        // Text over words in a file's own time: no timing, no layer.
        Op::FixWords {
            asset_id,
            start_us,
            end_us,
            text,
        } => nothing(project_ops::fix_words(
            p,
            asset_id,
            *start_us,
            *end_us,
            text.as_deref(),
        )),
        // A look over words in a file's own time: no timing, no layer.
        Op::SetWordLook {
            asset_id,
            ranges,
            look,
        } => nothing(project_ops::set_word_look(p, asset_id, ranges, look)),
        Op::SetBackground { background } => nothing(project_ops::set_background(p, background)),
        Op::SetAudio { audio } => nothing(project_ops::set_audio(p, audio)),
        Op::AddAssets { assets } => nothing(project_ops::add_assets(p, assets)),
        Op::AddRecording { recording } => nothing(project_ops::add_recording(p, recording)),
        Op::RemoveAsset { id } => nothing(project_ops::remove_asset(p, id)),
        Op::AddLut { lut } => nothing(project_ops::add_lut(p, lut)),
        // Clip looks only.
        Op::RemoveLut { id } => nothing(project_ops::remove_lut(p, id)),
        Op::AddTrack { id, kind, index } => {
            nothing(project_ops::add_track(p, id.as_deref(), *kind, *index))
        }
        Op::RemoveTrack { id } => nothing(project_ops::remove_track(p, id)),
        Op::MoveTrack { id, index } => nothing(project_ops::move_track(p, id, *index)),
        Op::UpdateTrack { id, patch } => nothing(project_ops::update_track(p, id, patch)),
        Op::InsertClip { clip, index } => main(main_ops::insert_clip(p, clip, *index)),
        Op::MoveClip { id, index } => main(main_ops::move_clip(p, id, *index, places)),
        Op::TrimClip {
            id,
            edge,
            source_us,
        } => main(main_ops::trim_clip(p, id, *edge, *source_us, places)),
        Op::Split { at_us } => main(main_ops::split(p, *at_us)),
        Op::DeleteRanges { ranges } => main(main_ops::delete_ranges(p, ranges)),
        Op::RemoveClip { id } => main(main_ops::remove_clip(p, id, places)),
        // Speed changes a clip's length; volume, fades, look, camera, framing and effects change no timing.
        Op::UpdateClip { id, patch } if patch.speed.is_some() => {
            main(main_ops::update_clip(p, id, patch, places))
        }
        Op::UpdateClip { id, patch } => nothing(main_ops::update_clip(p, id, patch, places)),
        Op::AddItem {
            track,
            item,
            attach,
        } => layer(
            item_ops::add_item(p, track, item, *attach, places),
            &item.id,
        ),
        // Length, layout, content and effects, and maybe another track: never its anchor or start. Kept in its slot,
        // it touched nothing settling reads.
        Op::UpdateItem { id, patch } => item_ops::update_item(p, id, patch, places).map(|kept| {
            if kept {
                Touched::Nothing
            } else {
                Touched::Placed(vec![id.clone()])
            }
        }),
        Op::MoveItem {
            id,
            track_id,
            start_us,
        } => layer(
            item_ops::move_item(p, id, track_id.as_deref(), *start_us, places),
            id,
        ),
        Op::TrimItem { id, edge, at_us } => {
            layer(item_ops::trim_item(p, id, *edge, *at_us, places), id)
        }
        Op::SplitItem { id, at_us } => item_ops::split_item(p, id, *at_us, places)
            .map(|second| Touched::Layers(vec![id.clone(), second])),
        Op::RemoveItem { id } => nothing(item_ops::remove_item(p, id, places)),
        Op::SetAttach { id, attached } => layer(item_ops::set_attach(p, id, *attached, places), id),
        // The clip's camera mode or its own mix of the source changes too, which is no timing.
        Op::DetachSource { clip_id, source_id } => {
            detach::detach_source(p, clip_id, source_id, places).map(Touched::Layers)
        }
        // The clip's mute changes too, which is no timing.
        Op::DetachSound { clip_id } => {
            detach::detach_sound(p, clip_id, places).map(Touched::Layers)
        }
        // A source's mix, name or sync: no timing; a sync change slips its separated sound layers, and those cut back
        // at their start have new starts and anchors.
        Op::UpdateRecordingSource {
            recording_id,
            source_id,
            patch,
        } => recording_ops::update_source(p, recording_id, source_id, patch).map(|cut| {
            if cut.is_empty() {
                Touched::Nothing
            } else {
                Touched::Layers(cut)
            }
        }),
        // A clip's own mix, and markers: no timing, no layer.
        Op::SetClipSourceMix {
            clip_id,
            source_id,
            mix,
        } => nothing(recording_ops::set_clip_source_mix(
            p,
            places,
            clip_id,
            source_id,
            mix.as_ref(),
        )),
        // A clip's source times move onto the recording clock and its layers' anchors with them: nothing moves,
        // but every anchor is read again, so it settles as a main op.
        Op::UseRecording {
            clip_id,
            recording_id,
        } => main(sync_ops::use_recording(p, clip_id, recording_id, places)),
        Op::AddRecordingSource {
            recording_id,
            source,
        } => nothing(sync_ops::add_recording_source(p, recording_id, source)),
        Op::AddMarker {
            recording_id,
            marker,
        } => nothing(recording_ops::add_marker(p, recording_id, marker)),
        Op::RemoveMarker { recording_id, id } => {
            nothing(recording_ops::remove_marker(p, recording_id, id))
        }
        Op::AddEffect {
            target,
            effect,
            index,
        } => nothing(effect_ops::add_effect(p, target, effect, *index, places)),
        Op::UpdateEffect { target, id, patch } => {
            nothing(effect_ops::update_effect(p, target, id, patch, places))
        }
        Op::RemoveEffect { target, id } => {
            nothing(effect_ops::remove_effect(p, target, id, places))
        }
        Op::MoveEffect { target, id, index } => {
            nothing(effect_ops::move_effect(p, target, id, *index, places))
        }
        // A transition doesn't move clips: their lengths and starts stay as they are.
        Op::SetTransition {
            clip_id,
            transition,
        } => nothing(main_ops::set_transition(
            p,
            clip_id,
            transition.as_ref(),
            places,
        )),
        // How the cursor draws: no timing, no layer.
        Op::SetCursorStyle { target, style } => nothing(project_ops::set_cursor_style(
            p,
            places,
            target,
            style.as_ref(),
        )),
    }
}

pub(crate) const LOCKED: &str = code!("model.track.locked");

/// Refuse op-supplied times outside `0..=MAX_TIME_US` before any arithmetic uses them.
fn check_time(t: Us) -> Result<()> {
    if (0..=MAX_TIME_US).contains(&t) {
        Ok(())
    } else {
        Err(invalid(code!("model.project.timeOutside")))
    }
}

/// Refuse op-supplied lengths outside `0..=MAX_TIME_US` before any arithmetic uses them.
fn check_length(d: Us) -> Result<()> {
    check_time(d).map_err(|_| invalid(code!("model.project.badLength")))
}

/// [`normalize`] as `apply_ops` runs it: after a batch's first op and after ops that change the main track's timing.
fn whole(p: &mut Project) {
    #[cfg(test)]
    tests::WHOLE_PASSES.with(|n| n.set(n.get() + 1));
    normalize(p);
}

/// [`whole`] after a speed change, when the clips are where they were: anchors resolve through the batch's clip index
/// and its starts (rebuilt, O(clips) of additions) instead of an index of every clip built and hashed per pass. The
/// same result as [`normalize`].
fn whole_indexed(p: &mut Project, places: &mut Places) {
    #[cfg(test)]
    tests::WHOLE_PASSES.with(|n| n.set(n.get() + 1));
    places.timing_changed();
    let Timeline { main, tracks } = &mut p.timeline;
    for item in tracks.iter_mut().flat_map(|t| t.items.iter_mut()) {
        if let Some(attach) = &item.attach {
            let located = places.locate(main, attach);
            follow(item, located);
        }
    }
    resolve_overlaps(p);
}

/// Re-derive attached starts, keep items sorted, and move colliding items to free tracks. O(clips + layers),
/// allocating only the clip index (and what a bump needs).
fn normalize(p: &mut Project) {
    let Timeline { main, tracks } = &mut p.timeline;
    let index = MainIndex::new(main);
    for item in tracks.iter_mut().flat_map(|t| t.items.iter_mut()) {
        if let Some(attach) = &item.attach {
            follow(item, index.locate(attach));
        }
    }
    resolve_overlaps(p);
}

/// [`normalize`] after an op of a batch whose project was normalised before it, from what the op [`Touched`]: the
/// same result at a fraction of the cost. Every other layer is unchanged by the whole-project pass: its anchor resolves
/// where it did, and its track, in order before the op and not changed by it, stays as it is. So a layer op re-derives
/// the starts of the layers it touched (O(1) each through `places`) and looks for overlaps around them only: O(1) per
/// layer unless something has to be bumped.
fn settle(p: &mut Project, touched: &Touched, places: &mut Places) {
    let ids = match touched {
        Touched::Main => return whole(p),
        Touched::Nothing => return,
        Touched::Layers(ids) => {
            follow_layers(p, ids, places);
            ids
        }
        Touched::Placed(ids) => ids,
    };
    if places.unindexed() {
        // The tracks hold an id twice: settle as the whole pass's order step does, every track looked at.
        return resolve_overlaps(p);
    }
    order_around(p, ids, places);
}

/// Each touched layer starts where its anchor now plays (`locate_one`'s rules, through the batch's index).
fn follow_layers(p: &mut Project, ids: &[Id], places: &mut Places) {
    if places.unindexed() {
        let Timeline { main, tracks } = &mut p.timeline;
        for item in tracks.iter_mut().flat_map(|t| t.items.iter_mut()) {
            if ids.contains(&item.id) {
                if let Some(attach) = &item.attach {
                    follow(item, locate_one(main, attach));
                }
            }
        }
        return;
    }
    for id in ids {
        let Some((t, i)) = places.item(&p.timeline.tracks, id) else {
            continue;
        };
        let Timeline { main, tracks } = &mut p.timeline;
        let item = &mut tracks[t].items[i];
        if let Some(attach) = &item.attach {
            let located = places.locate(main, attach);
            follow(item, located);
        }
    }
}

/// [`resolve_overlaps`] when only the touched layers changed: a track is out of order only if a touched layer is out
/// of order with a neighbour (the rest of the track was in order and is unchanged), or if layers were bumped onto it,
/// and only such a track is bumped in turn, bottom to top as the whole pass goes. O(touched × tracks) unless something
/// is bumped.
fn order_around(p: &mut Project, ids: &[Id], places: &mut Places) {
    let mut bumped_onto: Vec<Id> = Vec::new();
    let mut i: usize = 0;
    while i < p.timeline.tracks.len() {
        let tracks = &p.timeline.tracks;
        let out = if bumped_onto.contains(&tracks[i].id) {
            !in_order(&tracks[i].items)
        } else {
            ids.iter().any(|id| {
                places
                    .item(tracks, id)
                    .is_some_and(|(t, k)| t == i && !in_order_at(&tracks[t].items, k))
            })
        };
        if out {
            bumped_onto.extend(bump_overlaps(p, i));
        }
        i = i.saturating_add(1);
    }
}

/// Whether the layer at `k` is in order with both its neighbours (`in_order` for those two pairs).
fn in_order_at(items: &[Item], k: usize) -> bool {
    let pair = |a: &Item, b: &Item| a.start_us <= b.start_us && !collides(a, b);
    (k == 0 || pair(&items[k - 1], &items[k]))
        && items.get(k + 1).is_none_or(|b| pair(&items[k], b))
}

/// An attached layer starts where its anchor now plays, or is detached when nothing plays it.
fn follow(item: &mut Item, located: Option<(usize, Us)>) {
    match located {
        Some((_, start)) => item.start_us = start,
        None => item.attach = None,
    }
}

/// Sort each track and bump items that overlap the previous one by more than [`OVERLAP_TOLERANCE_US`] (smaller
/// overlaps count as touching, as in validation). A track that is already in order is only looked at (O(layers), no
/// moves): the pass would leave it exactly as it is.
fn resolve_overlaps(p: &mut Project) {
    let mut i: usize = 0;
    while i < p.timeline.tracks.len() {
        if !in_order(&p.timeline.tracks[i].items) {
            bump_overlaps(p, i);
        }
        i = i.saturating_add(1);
    }
}

/// Whether `later` (starting no earlier than `earlier`) overlaps it by more than the tolerance.
fn collides(earlier: &Item, later: &Item) -> bool {
    later.start_us.saturating_add(OVERLAP_TOLERANCE_US) < earlier.end_us()
}

/// Sorted by start and free of collisions: what [`bump_overlaps`] leaves unchanged (a stable sort of a sorted track
/// keeps its order, and with nothing bumped each layer's previous kept one is its neighbour).
fn in_order(items: &[Item]) -> bool {
    items
        .windows(2)
        .all(|w| w[0].start_us <= w[1].start_us && !collides(&w[0], &w[1]))
}

/// Sort track `i` and move each layer colliding with the one before it to a track above ([`place_above`], which looks at
/// every layer: the tracks above may not be in order yet). Returns the ids of the tracks the bumped layers went to.
fn bump_overlaps(p: &mut Project, i: usize) -> Vec<Id> {
    let (outside, tracks) = split_tracks(p);
    let items = &mut tracks[i].items;
    // A stable sort leaves a sorted track as it is: skip it, and its copying of every layer.
    if !items.windows(2).all(|w| w[0].start_us <= w[1].start_us) {
        items.sort_by_key(|item| item.start_us);
    }
    // Each layer that collides with the last one kept is bumped: decided on times alone, then only the layers from the
    // first bumped one on move (each 816 bytes), instead of the whole track.
    let mut at: Vec<usize> = Vec::new();
    let mut last: Option<usize> = None;
    for k in 0..items.len() {
        if last.is_some_and(|l| collides(&items[l], &items[k])) {
            at.push(k);
        } else {
            last = Some(k);
        }
    }
    let mut bumped: Vec<Item> = Vec::with_capacity(at.len());
    if let Some(&first) = at.first() {
        let mut n = first;
        let mut next = at.iter().peekable();
        bumped.extend(items.extract_if(first.., |_| {
            let out = next.peek().is_some_and(|&&k| k == n);
            if out {
                next.next();
            }
            n += 1;
            out
        }));
    }
    let mut onto = Vec::new();
    if !bumped.is_empty() {
        // Bumped layers still waiting to be placed are on no track yet: reserve their ids too.
        let waiting: Vec<Id> = bumped.iter().map(|item| item.id.clone()).collect();
        let reserved = |id: &str| outside.contains(id) || waiting.iter().any(|w| w == id);
        for item in bumped {
            let (j, _) = place_above(tracks, i, item, &reserved, false, false);
            onto.push(tracks[j].id.clone());
        }
    }
    onto
}

/// The project's ids outside its tracks (the project, its media, recordings, LUTs and clips), borrowed beside the
/// tracks so a new track's id can avoid them without copying them into a set.
pub(crate) struct Outside<'a> {
    project: &'a str,
    assets: &'a [Asset],
    recordings: &'a [Recording],
    luts: &'a [ColorLut],
    main: &'a [Clip],
}

impl<'a> Outside<'a> {
    pub(crate) fn of(p: &'a Project) -> Self {
        Self {
            project: &p.id,
            assets: &p.assets,
            recordings: &p.recordings,
            luts: &p.luts,
            main: &p.timeline.main,
        }
    }

    /// O(ids outside the tracks), no allocation: new ids are checked a few at a time.
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.project == id
            || self.assets.iter().any(|a| a.id == id)
            || self.recordings.iter().any(|r| r.id == id)
            || self.luts.iter().any(|l| l.id == id)
            || self.main.iter().any(|c| c.id == id)
    }
}

/// The ids outside the tracks, and the tracks to change.
pub(crate) fn split_tracks(p: &mut Project) -> (Outside<'_>, &mut Vec<Track>) {
    let Project {
        id,
        assets,
        recordings,
        luts,
        timeline,
        ..
    } = p;
    let outside = Outside {
        project: id,
        assets,
        recordings,
        luts,
        main: &timeline.main,
    };
    (outside, &mut timeline.tracks)
}

/// Whether any of the project's ids is `id`. O(ids), no allocation: for naming what an edit creates, a candidate or
/// two at a time.
pub(crate) fn id_taken(p: &Project, id: &str) -> bool {
    Outside::of(p).contains(id)
        || p.timeline
            .tracks
            .iter()
            .any(|t| t.id == id || t.items.iter().any(|item| item.id == id))
}

/// Put `item` on the first unlocked, visible track above `from` (same kind) where it fits, or a new track right above.
/// With `music_apart` (a layer added or moved by an op, `item_ops::place`), a sound effect never goes onto one that
/// holds music; the normalise's bumps (`bump_overlaps`) leave it off, as their twins in `speed_lanes` and the test
/// reference do, so all three settle alike. A new track's id is derived from the item's (`{item}-track`, numbered if
/// taken), so repeating an edit repeats the id. `reserved` says which ids outside `tracks` are taken
/// ([`Outside`]); the new id avoids those and every track and item id. With `in_order`, every track above is known to
/// be in order, so fitting is a binary search ([`fits_in_order`]). Returns where the item went.
pub(crate) fn place_above(
    tracks: &mut Vec<Track>,
    from: usize,
    item: Item,
    reserved: &dyn Fn(&str) -> bool,
    in_order: bool,
    music_apart: bool,
) -> (usize, usize) {
    let kind = tracks[from].kind;
    let (start, end) = (item.start_us, item.end_us());
    // A sound effect added or moved by an op never goes up onto a track that holds music.
    let sound = music_apart && kind == TrackKind::Audio && !crate::media::music::is_music(&item);
    let above = from.saturating_add(1).min(tracks.len());
    let fit = |track: &Track| {
        if in_order {
            fits_in_order(track, start, end, None)
        } else {
            fits(track, start, end, None)
        }
    };
    // Shown, unlocked, of the kind, and for a sound effect holding no music.
    let open = |track: &Track| {
        if track.kind != kind || track.locked || track.hidden {
            return false;
        }
        !sound || !track.items.iter().any(crate::media::music::is_music)
    };
    if let Some(j) = (above..tracks.len()).find(|&j| open(&tracks[j]) && fit(&tracks[j])) {
        return (j, insert_sorted(&mut tracks[j], item));
    }
    let mut track = Track::new(kind, default_track_name(kind, tracks));
    track.id = fresh_id(format!("{}-track", item.id), |id| {
        reserved(id)
            || tracks
                .iter()
                .any(|t| t.id == id || t.items.iter().any(|item| item.id == id))
    });
    track.items.push(item);
    tracks.insert(above, track);
    (above, 0)
}

/// [`fits`] on a track that is in order (`in_order`), in O(log layers) and with exactly its answer: of the layers
/// starting at or after `start`, the first bounds `end`; of those starting before it, only the last can reach past
/// `start` (each earlier one ends, give or take the tolerance, before the next one starts). Layers named `ignore` are
/// passed over, as `fits` does.
pub(crate) fn fits_in_order(track: &Track, start: Us, end: Us, ignore: Option<&str>) -> bool {
    if end <= start {
        return false;
    }
    let items = &track.items;
    let at = items.partition_point(|item| item.start_us < start);
    let counted = |item: &&Item| Some(item.id.as_str()) != ignore;
    let after = items[at..]
        .iter()
        .find(counted)
        .is_none_or(|item| end <= item.start_us.saturating_add(OVERLAP_TOLERANCE_US));
    let before = items[..at]
        .iter()
        .rev()
        .find(counted)
        .is_none_or(|item| start.saturating_add(OVERLAP_TOLERANCE_US) >= item.end_us());
    after && before
}

/// Every id in the project, copied into a set for an edit that names many things ([`IdPool`]). Generated ids avoid
/// all of them, so validation never sees a duplicate.
fn all_ids(p: &Project) -> HashSet<Id> {
    std::iter::once(&p.id)
        .chain(p.assets.iter().map(|a| &a.id))
        .chain(p.recordings.iter().map(|r| &r.id))
        .chain(p.luts.iter().map(|l| &l.id))
        .chain(p.timeline.main.iter().map(|c| &c.id))
        .chain(
            p.timeline
                .tracks
                .iter()
                .flat_map(|t| std::iter::once(&t.id).chain(t.items.iter().map(|item| &item.id))),
        )
        .cloned()
        .collect()
}

/// `base` itself if it isn't `taken`, else [`derived_id`]`(base)`.
pub(crate) fn fresh_id(base: String, mut taken: impl FnMut(&str) -> bool) -> Id {
    if taken(&base) {
        derived_id(&base, taken)
    } else {
        base
    }
}

/// `{base}-{n}` with the smallest `n >= 2` that isn't `taken`. Pieces and tracks created by an
/// edit get ids derived this way (not random), so a dry run and a commit produce the same project.
pub(crate) fn derived_id(base: &str, mut taken: impl FnMut(&str) -> bool) -> Id {
    let mut n: u64 = 2;
    loop {
        let id = format!("{base}-{n}");
        if !taken(&id) {
            return id;
        }
        n = n.saturating_add(1);
    }
}

/// Ids in use while an edit runs, for naming what it creates. Remembers where each base's
/// [`derived_id`] search stopped, so deriving many ids from one base stays linear. Ids are only
/// ever added, so every id is the one [`derived_id`] would pick.
pub(crate) struct IdPool {
    taken: HashSet<Id>,
    next: HashMap<String, u64>,
}

impl IdPool {
    /// Every id in `p` is taken.
    pub(crate) fn of(p: &Project) -> Self {
        Self {
            taken: all_ids(p),
            next: HashMap::new(),
        }
    }

    /// Room for `n` more ids, so naming thousands of pieces doesn't regrow (and rehash) the set as it goes.
    pub(crate) fn reserve(&mut self, n: usize) {
        self.taken.reserve(n);
    }

    /// [`derived_id`]`(base)`, now taken.
    pub(crate) fn derive(&mut self, base: &str) -> Id {
        let mut n = self.next.get(base).copied().unwrap_or(2);
        loop {
            let id = format!("{base}-{n}");
            n = n.saturating_add(1);
            if self.taken.insert(id.clone()) {
                // A cut deriving thousands of pieces from one base copies the base once.
                match self.next.get_mut(base) {
                    Some(next) => *next = n,
                    None => {
                        self.next.insert(base.to_string(), n);
                    }
                }
                return id;
            }
        }
    }
}

/// Insert `item` after every layer starting no later than it; returns its index.
pub(crate) fn insert_sorted(track: &mut Track, item: Item) -> usize {
    let at = track
        .items
        .partition_point(|other| other.start_us <= item.start_us);
    track.items.insert(at, item);
    at
}

/// "Overlay 1", "Audio 2", "Zoom", "Zoom 2", …: the first number whose name no track uses yet.
pub(crate) fn default_track_name(kind: TrackKind, tracks: &[Track]) -> String {
    default_name(kind, |candidate| tracks.iter().any(|t| t.name == candidate))
}

/// [`default_track_name`] with the names in use told by `used`.
pub(crate) fn default_name(kind: TrackKind, used: impl Fn(&str) -> bool) -> String {
    let name = |n: u64| match kind {
        TrackKind::Overlay => format!("Overlay {n}"),
        TrackKind::Audio => format!("Audio {n}"),
        TrackKind::Zoom if n == 1 => "Zoom".into(),
        TrackKind::Zoom => format!("Zoom {n}"),
    };
    let mut n: u64 = 1;
    loop {
        let candidate = name(n);
        if !used(&candidate) {
            return candidate;
        }
        n = n.saturating_add(1);
    }
}

#[cfg(test)]
mod tests;
