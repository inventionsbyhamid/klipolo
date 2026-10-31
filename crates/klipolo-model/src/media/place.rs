//! Sounds and pictures let go on the timeline (one dragged from the library, or several from Finder): each placed as
//! the library's "+" places it ([`sound::add_sound_ops`], [`image::add_image_ops`]), on the track it was let go on when
//! that track takes it, one after another from the moment it was let go at, as one undo step.

use crate::error::{invalid, ModelError, Result};
use crate::ids::{new_id, Id};
use crate::messages::code;
use crate::ops::{Op, TrackTarget};
use crate::project::{Asset, AssetKind, Project};
use crate::time::Us;
use crate::timeline::TrackKind;

use super::music::is_music;
use super::{image, sound};

/// Which track a drop names: none (wherever there is room), the track it was let go on, or a new track of its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Onto {
    Auto,
    Track(Id),
    New(TrackKind),
}

/// What `place_ops` makes: the ops, and the files that come into the project without a layer, each with why (by its
/// index in the files asked for).
#[derive(Debug)]
pub struct Placed {
    pub ops: Vec<Op>,
    pub unplaced: Vec<(usize, ModelError)>,
}

/// The ops that place `assets` (sounds and pictures) from `at_us`, one after another: each starts where the one before
/// it ends. A sound goes where `sound::add_sound_ops` puts it and a picture where `image::add_image_ops` does, attached
/// to the clip under it; on `onto`'s track when that track takes it (its kind, shown and unlocked, and for a sound
/// effect holding no music), else wherever there is room. A new track is made once, for the files of its kind. A file
/// that can't be placed (no sound, or no room left at or past the video's end) still comes into the project, without a
/// layer, and is named in `unplaced`. Refused for a video, and with the first file's reason when none is placed.
pub fn place_ops(p: &Project, assets: &[Asset], at_us: Us, onto: &Onto) -> Result<Placed> {
    if assets.iter().any(|a| a.kind == AssetKind::Video) {
        return Err(invalid(code!("model.image.notImage")));
    }
    // Each file the project doesn't have yet, once (the same pack sound let go twice is one file).
    let mut missing: Vec<Asset> = Vec::new();
    for asset in assets {
        if p.asset(&asset.id).is_none() && !missing.iter().any(|m| m.id == asset.id) {
            missing.push(asset.clone());
        }
    }
    let mut ops = Vec::new();
    let mut now = p.clone();
    if !missing.is_empty() {
        let add = Op::AddAssets { assets: missing };
        now = crate::apply_ops(&now, std::slice::from_ref(&add))?;
        ops.push(add);
    }
    let mut track = match onto {
        Onto::Track(id) => Some(id.clone()),
        Onto::New(_) | Onto::Auto => None,
    };
    let mut at = at_us;
    let mut placed = 0;
    let mut unplaced = Vec::new();
    for (index, asset) in assets.iter().enumerate() {
        let kind = if asset.kind == AssetKind::Audio {
            TrackKind::Audio
        } else {
            TrackKind::Overlay
        };
        let built = match asset.kind {
            AssetKind::Audio => sound::add_sound_ops(&now, asset, at),
            _ => image::add_image_ops(&now, asset, at),
        };
        let mut built = match built {
            Ok(built) => built,
            Err(error) => {
                unplaced.push((index, error));
                continue;
            }
        };
        if onto == &Onto::New(kind) && track.is_none() {
            let id = new_id();
            let add = Op::AddTrack {
                id: Some(id.clone()),
                kind,
                index: Some(now.timeline.tracks.len()),
            };
            now = crate::apply_ops(&now, std::slice::from_ref(&add))?;
            ops.push(add);
            track = Some(id);
        }
        // A sound effect never goes on a track that holds music (`auto_track` keeps it off them too).
        let takes = |id: &Id| {
            now.timeline.tracks.iter().any(|t| {
                &t.id == id
                    && t.kind == kind
                    && !t.locked
                    && !t.hidden
                    && !(kind == TrackKind::Audio && t.items.iter().any(is_music))
            })
        };
        if let Some(id) = track.as_ref().filter(|id| takes(id)) {
            for op in &mut built {
                if let Op::AddItem { track: target, .. } = op {
                    *target = TrackTarget::Track { id: id.clone() };
                }
            }
        }
        now = crate::apply_ops(&now, &built)?;
        for op in &built {
            if let Op::AddItem { item, .. } = op {
                at = item.end_us();
                placed += 1;
            }
        }
        ops.extend(built);
    }
    if placed == 0 {
        return Err(unplaced
            .into_iter()
            .next()
            .map_or_else(|| invalid(code!("model.sound.noRoom")), |(_, error)| error));
    }
    Ok(Placed { ops, unplaced })
}
