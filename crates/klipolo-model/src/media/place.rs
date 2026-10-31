//! Sounds and pictures let go on the timeline (one dragged from the library, or several from Finder): each placed as
//! the library's "+" places it ([`sound::add_sound_ops`], [`image::add_image_ops`]), on the track it was let go on when
//! that track takes it, one after another from the moment it was let go at, as one undo step.

use crate::error::{invalid, Result};
use crate::ids::{new_id, Id};
use crate::messages::code;
use crate::ops::{Op, TrackTarget};
use crate::project::{Asset, AssetKind, Project};
use crate::time::Us;
use crate::timeline::TrackKind;

use super::{image, sound};

/// Which track a drop names: none (wherever there is room), the track it was let go on, or a new track of its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Onto {
    Auto,
    Track(Id),
    New(TrackKind),
}

/// The ops that place `assets` (sounds and pictures) from `at_us`, one after another: each starts where the one before
/// it ends. A sound goes where `sound::add_sound_ops` puts it and a picture where `image::add_image_ops` does, attached
/// to the clip under it; on `onto`'s track when that track takes its kind (shown and unlocked), else wherever there is
/// room. A new track is made once, for the files of its kind. A file with no room left (at or past the video's end)
/// still comes into the project, without a layer. Refused for a video, and when no file is placed at all (no room).
pub fn place_ops(p: &Project, assets: &[Asset], at_us: Us, onto: &Onto) -> Result<Vec<Op>> {
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
    for asset in assets {
        let kind = if asset.kind == AssetKind::Audio {
            TrackKind::Audio
        } else {
            TrackKind::Overlay
        };
        let built = match asset.kind {
            AssetKind::Audio => sound::add_sound_ops(&now, asset, at),
            _ => image::add_image_ops(&now, asset, at),
        };
        let Ok(mut built) = built else {
            continue;
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
        let takes = |id: &Id| {
            now.timeline
                .tracks
                .iter()
                .any(|t| &t.id == id && t.kind == kind && !t.locked && !t.hidden)
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
        return Err(invalid(code!("model.sound.noRoom")));
    }
    Ok(ops)
}
