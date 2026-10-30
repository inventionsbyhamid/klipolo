//! Tests only: the whole-project normalise as it was before batches settled incrementally (`3823436`), kept verbatim
//! as a second reference. It sorts and bumps **every** track and clones the reserved ids into a set, so it shares
//! none of the new pass's shortcuts (`in_order`, `collides`, `Outside`): a later change that breaks one of them can't
//! hide behind a reference that has the same change. Only the anchor lookup (`MainIndex::locate`) is shared, whose
//! rules were only moved into `query::pick`, not changed.

use std::collections::HashSet;

use super::{default_track_name, fresh_id, insert_sorted};
use crate::ids::Id;
use crate::project::Project;
use crate::query::{fits, MainIndex};
use crate::time::OVERLAP_TOLERANCE_US;
use crate::timeline::{Item, Track};

/// Re-derive attached starts, keep items sorted, and move colliding items to free tracks.
pub(crate) fn normalize_before_pa1(p: &mut Project) {
    let main = MainIndex::new(&p.timeline.main);
    for track in p.timeline.tracks.iter_mut() {
        for item in track.items.iter_mut() {
            if let Some(attach) = &item.attach {
                match main.locate(attach) {
                    Some((_, start)) => item.start_us = start,
                    None => item.attach = None,
                }
            }
        }
    }
    let reserved = ids_outside_tracks(p);
    resolve_overlaps(&mut p.timeline.tracks, &reserved);
}

/// Sort each track and bump items that overlap the previous one by more than
/// [`OVERLAP_TOLERANCE_US`] (smaller overlaps count as touching, as in validation).
/// `reserved` holds the project's ids outside `tracks` (see [`place_above`]).
fn resolve_overlaps(tracks: &mut Vec<Track>, reserved: &HashSet<Id>) {
    let mut i: usize = 0;
    while i < tracks.len() {
        tracks[i].items.sort_by_key(|item| item.start_us);
        let mut kept: Vec<Item> = Vec::with_capacity(tracks[i].items.len());
        let mut bumped: Vec<Item> = Vec::new();
        for item in std::mem::take(&mut tracks[i].items) {
            let collides = kept.last().is_some_and(|prev| {
                item.start_us.saturating_add(OVERLAP_TOLERANCE_US) < prev.end_us()
            });
            if collides {
                bumped.push(item);
            } else {
                kept.push(item);
            }
        }
        tracks[i].items = kept;
        if !bumped.is_empty() {
            // Bumped layers still waiting to be placed are on no track yet: reserve their ids too.
            let mut taken = reserved.clone();
            taken.extend(bumped.iter().map(|item| item.id.clone()));
            for item in bumped {
                place_above(tracks, i, item, &taken);
            }
        }
        i = i.saturating_add(1);
    }
}

/// Put `item` on the first unlocked, visible track above `from` (same kind) where it fits, or a
/// new track right above. A new track's id is derived from the item's (`{item}-track`, numbered if
/// taken), so repeating an edit repeats the id. `reserved` holds the project's ids outside `tracks`
/// ([`ids_outside_tracks`]); the new id avoids those and every track and item id.
fn place_above(tracks: &mut Vec<Track>, from: usize, item: Item, reserved: &HashSet<Id>) -> usize {
    let kind = tracks[from].kind;
    let (start, end) = (item.start_us, item.end_us());
    let above = from.saturating_add(1).min(tracks.len());
    if let Some(j) = (above..tracks.len()).find(|&j| {
        let track = &tracks[j];
        track.kind == kind && !track.locked && !track.hidden && fits(track, start, end, None)
    }) {
        insert_sorted(&mut tracks[j], item);
        return j;
    }
    let mut track = Track::new(kind, default_track_name(kind, tracks));
    track.id = fresh_id(format!("{}-track", item.id), |id| {
        reserved.contains(id)
            || tracks
                .iter()
                .any(|t| t.id == id || t.items.iter().any(|item| item.id == id))
    });
    track.items.push(item);
    tracks.insert(above, track);
    above
}

/// Ids of the project, its media, recordings, LUTs and clips: everything but tracks and items.
fn ids_outside_tracks(p: &Project) -> HashSet<Id> {
    std::iter::once(&p.id)
        .chain(p.assets.iter().map(|a| &a.id))
        .chain(p.recordings.iter().map(|r| &r.id))
        .chain(p.luts.iter().map(|l| &l.id))
        .chain(p.timeline.main.iter().map(|c| &c.id))
        .cloned()
        .collect()
}
