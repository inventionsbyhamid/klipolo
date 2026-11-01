//! `setLayer` on a layer of the additions' working copy: its layout (box, rotation, opacity, corners, blend), a video
//! layer's crop and Fit or Fill, and its place in the stack, as the layer card and the timeline's Bring forward and
//! Send back change them, in one step.
//! - A box is the primary format's place, and every other format follows it from there (`layout::resolve_rect`, anchored
//!   to the nearest of the nine anchors, `anchor_for_rect`), as one place for every shape of the video. A title template
//!   fills the frame, so it takes no box.
//! - Forward and back move the layer onto the nearest unlocked track of its kind above or below, at the same time, as
//!   ⌥↑ and ⌥↓ do; with none there, it is refused back to the model.

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ids::Id;
use klipolo_model::layout::anchor_for_rect;
use klipolo_model::media::music::{MUSIC_DUCK_DB, runs};
use klipolo_model::motion::Blend;
use klipolo_model::ops::{ClipPatch, Edge, ItemPatch, Op};
use klipolo_model::timeline::{ItemContent, TrackKind};

use super::animate::noun;
use super::motion::drawn;
use super::{Compiler, Edit, Refused, label, to_model};
use crate::tools::{LayerChange, Order};

/// `setLayer` on the layer `id` of `work`, named `said`: its layout and a video layer's crop and framing in one
/// `UpdateItem`, then its move up or down the stack, to a new start (a time the context shows, as a media tool's start
/// is checked) and to a new length. Taking it away (`remove`) goes alone: a removal the compiler proposes.
pub(super) fn set_layer(
    c: &Compiler,
    work: &Project,
    (id, said): (&Id, &str),
    change: &LayerChange,
) -> Result<Edit, Refused> {
    let item = drawn(work, (id, said), "setLayer")?;
    if change.remove {
        if *change
            != (LayerChange {
                remove: true,
                ..LayerChange::default()
            })
        {
            return Err(to_model(
                "setLayer: remove takes a layer away, so it goes alone: leave the other keys out."
                    .to_owned(),
            ));
        }
        return Ok(Edit {
            ops: vec![Op::RemoveItem { id: id.clone() }],
            label: label("removeLayer", vec![("layer", ErrorParam::from(noun(item)))]),
        });
    }
    if change.separate {
        return Err(to_model(format!(
            "setLayer: separate is for imported clips (c1), and {said} is a layer. Leave it out."
        )));
    }
    if change.fade_in_us.is_some() || change.fade_out_us.is_some() || change.duck.is_some() {
        return Err(to_model(format!(
            "setLayer: fadeIn, fadeOut and duck are for clips and music, and {said} is a layer. Fade a layer in or \
             out with setMotion's presets."
        )));
    }
    let template = matches!(&item.content, ItemContent::Generator { source, .. }
        if source.is_scene() && source.as_str() != klipolo_model::media::text_motion::SOURCE);
    if change.rect.is_some() && template {
        return Err(to_model(format!(
            "setLayer: {said} is a title template that fills the frame, so it takes no box. Leave box null."
        )));
    }
    let mut layout = item.layout.clone();
    if let Some(rect) = change.rect {
        layout.rect = rect;
        layout.anchor = anchor_for_rect(&rect);
        layout.overrides.clear();
    }
    if let Some(r) = change.rotation {
        layout.rotation = r;
    }
    if let Some(o) = change.opacity {
        layout.opacity = o;
    }
    if let Some(c) = change.corners {
        layout.radius = c;
    }
    if let Some(b) = change.blend {
        layout.blend = (b != Blend::Normal).then_some(b);
    }
    let mut content = item.content.clone();
    if change.crop.is_some() || change.fill.is_some() {
        let ItemContent::Video { crop, fill, .. } = &mut content else {
            let field = if change.crop.is_some() {
                "crop"
            } else {
                "fill"
            };
            return Err(to_model(format!(
                "setLayer: {field} is for video layers (v1), and {said} is not one. Leave {field} null."
            )));
        };
        if let Some(c) = change.crop {
            *crop = (!c.is_none()).then_some(c);
        }
        if let Some(f) = change.fill {
            *fill = f;
        }
    }
    let mut ops = Vec::new();
    if layout != item.layout || content != item.content {
        ops.push(Op::UpdateItem {
            id: item.id.clone(),
            patch: ItemPatch {
                layout: (layout != item.layout).then_some(layout),
                content: (content != item.content).then_some(content),
                ..ItemPatch::default()
            },
        });
    }
    if let Some(order) = change.order {
        ops.push(step(work, id, said, order)?);
    }
    let start = match change.start_us {
        Some(at) => {
            let now = super::media::start(c, work, "setLayer", "start", Some(at))?;
            ops.push(Op::MoveItem {
                id: id.clone(),
                track_id: None,
                start_us: now,
            });
            now
        }
        None => item.start_us,
    };
    if let Some(length) = change.length_us {
        ops.push(Op::TrimItem {
            id: id.clone(),
            edge: Edge::End,
            at_us: start.saturating_add(length),
        });
    }
    let edit = Edit {
        ops,
        label: label("setLayer", vec![("layer", ErrorParam::from(noun(item)))]),
    };
    Ok(edit)
}

/// The move of layer `id` onto the nearest unlocked track of its kind in `order`'s direction (forward is a higher
/// track, drawn over), at the same time.
fn step(work: &Project, id: &Id, said: &str, order: Order) -> Result<Op, Refused> {
    let tracks = &work.timeline.tracks;
    let Some((at, item)) = (tracks.iter().enumerate())
        .find_map(|(t, track)| track.items.iter().find(|i| &i.id == id).map(|i| (t, i)))
    else {
        return Err(to_model(format!("setLayer: {said} is gone.")));
    };
    let kind = tracks[at].kind;
    let found = match order {
        Order::Forward => {
            (at + 1..tracks.len()).find(|&t| tracks[t].kind == kind && !tracks[t].locked)
        }
        Order::Back => (0..at)
            .rev()
            .find(|&t| tracks[t].kind == kind && !tracks[t].locked),
    };
    let Some(t) = found else {
        let (side, other) = match order {
            Order::Forward => ("in front", "above"),
            Order::Back => ("at the back", "below"),
        };
        return Err(to_model(format!(
            "setLayer: {said} is already {side}: no track of its kind is {other} it. Leave order null."
        )));
    };
    Ok(Op::MoveItem {
        id: id.clone(),
        track_id: Some(tracks[t].id.clone()),
        start_us: item.start_us,
    })
}

/// What `change` sets beyond a clip's or a song's fades and ducking, by key, for a refusal.
fn beyond_sound(change: &LayerChange, song: bool) -> Option<&'static str> {
    let c = change;
    [
        (c.rect.is_some(), "box"),
        (c.rotation.is_some(), "rotation"),
        (c.opacity.is_some(), "opacity"),
        (c.corners.is_some(), "corners"),
        (c.blend.is_some(), "blend"),
        (c.crop.is_some() && song, "crop"),
        (c.fill.is_some(), "fill"),
        (c.order.is_some(), "order"),
        (c.start_us.is_some(), "start"),
        (c.length_us.is_some(), "length"),
        (c.remove && !song, "remove"),
        (c.separate && song, "separate"),
        (c.duck.is_some() && !song, "duck"),
    ]
    .into_iter()
    .find_map(|(set, key)| set.then_some(key))
}

/// `setLayer` on the main track's clip `id`, named `said`: its fades, as the clip card's Fade in and Fade out set
/// them, its crop, and its sound put on a layer of its own (an imported clip's, as Separate sound does). A fade where a
/// transition already blends the cut is refused, as the card refuses it.
pub(super) fn set_clip_fades(
    work: &Project,
    (id, said): (&Id, &str),
    change: &LayerChange,
) -> Result<Edit, Refused> {
    if let Some(key) = beyond_sound(change, false) {
        return Err(to_model(format!(
            "setLayer: {said} is a clip, which takes only crop, separate, fadeIn and fadeOut here, not {key}. Use \
             setClip for its speed, volume or framing, trimClip or cutRange for its length."
        )));
    }
    let main = &work.timeline.main;
    let Some(at) = main.iter().position(|c| &c.id == id) else {
        return Err(to_model(format!("setLayer: {said} is gone.")));
    };
    let clip = &main[at];
    let into = at
        .checked_sub(1)
        .and_then(|i| main[i].transition_out.as_ref());
    if change.fade_in_us.is_some_and(|f| f > 0) && into.is_some() {
        return Err(to_model(format!(
            "setLayer: {said} starts with a transition from the clip before, so it can't fade in there. Remove the \
             transition first with setTransition."
        )));
    }
    if change.fade_out_us.is_some_and(|f| f > 0) && clip.transition_out.is_some() {
        return Err(to_model(format!(
            "setLayer: {said} ends with a transition into the next clip, so it can't fade out there. Remove the \
             transition first with setTransition."
        )));
    }
    let length = clip.duration_us();
    let fade_in = change.fade_in_us.unwrap_or(clip.fade_in_us);
    let fade_out = change.fade_out_us.unwrap_or(clip.fade_out_us);
    if fade_in.saturating_add(fade_out) > length {
        return Err(to_model(format!(
            "setLayer: {said} lasts {} s, so its fades together must fit in it.",
            crate::context::secs(length)
        )));
    }
    let crop = change
        .crop
        .filter(|c| (!c.is_none()).then_some(*c) != clip.crop);
    let fades = (fade_in, fade_out) != (clip.fade_in_us, clip.fade_out_us);
    let mut ops = Vec::new();
    if fades || crop.is_some() {
        ops.push(Op::UpdateClip {
            id: id.clone(),
            patch: ClipPatch {
                fade_in_us: change.fade_in_us.filter(|_| fades),
                fade_out_us: change.fade_out_us.filter(|_| fades),
                crop,
                ..ClipPatch::default()
            },
        });
    }
    if change.separate {
        let imported = matches!(
            clip.source,
            klipolo_model::timeline::ClipSource::Asset { .. }
        );
        if !imported {
            return Err(to_model(format!(
                "setLayer: {said} is a recording, whose sounds separateSound puts on tracks of their own."
            )));
        }
        ops.push(Op::DetachSound {
            clip_id: id.clone(),
        });
    }
    Ok(Edit {
        ops,
        label: label("setLayer", vec![("layer", ErrorParam::from("clip"))]),
    })
}

/// `setLayer` on the song whose first piece is `first`, named `said`: a fade in on its first piece, a fade out on its
/// last, and ducking under speech on every piece (at music's own depth, `MUSIC_DUCK_DB`, or off), as its card sets them.
pub(super) fn set_song_sound(
    work: &Project,
    (first, said): (&str, &str),
    change: &LayerChange,
) -> Result<Edit, Refused> {
    if let Some(key) = beyond_sound(change, true) {
        return Err(to_model(format!(
            "setLayer: {said} is music, which takes only fadeIn, fadeOut, duck and remove here, not {key}."
        )));
    }
    let song = (work.timeline.tracks.iter())
        .filter(|t| t.kind == TrackKind::Audio)
        .flat_map(|t| runs(t).into_iter().map(move |run| &t.items[run]))
        .find(|pieces| pieces.iter().any(|piece| piece.id == first));
    let Some(pieces) = song else {
        return Err(to_model(format!("setLayer: {said} is gone.")));
    };
    if change.remove {
        if *change
            != (LayerChange {
                remove: true,
                ..LayerChange::default()
            })
        {
            return Err(to_model(
                "setLayer: remove takes the music away, so it goes alone: leave the other keys out.".to_owned(),
            ));
        }
        return Ok(Edit {
            ops: (pieces.iter())
                .map(|piece| Op::RemoveItem {
                    id: piece.id.clone(),
                })
                .collect(),
            label: label("removeLayer", vec![("layer", ErrorParam::from("music"))]),
        });
    }
    let last = pieces.len() - 1;
    let mut ops = Vec::new();
    for (k, piece) in pieces.iter().enumerate() {
        let mut content = piece.content.clone();
        let ItemContent::Audio {
            fade_in_us,
            fade_out_us,
            duck,
            ..
        } = &mut content
        else {
            continue;
        };
        if let (0, Some(f)) = (k, change.fade_in_us) {
            *fade_in_us = f.min(piece.duration_us);
        }
        if k == last
            && let Some(f) = change.fade_out_us
        {
            *fade_out_us = f.min(piece.duration_us.saturating_sub(*fade_in_us));
        }
        if let Some(on) = change.duck {
            *duck = Some(if on { MUSIC_DUCK_DB } else { 0.0 });
        }
        if content != piece.content {
            ops.push(Op::UpdateItem {
                id: piece.id.clone(),
                patch: ItemPatch {
                    content: Some(content),
                    ..ItemPatch::default()
                },
            });
        }
    }
    Ok(Edit {
        ops,
        label: label("setLayer", vec![("layer", ErrorParam::from("music"))]),
    })
}

/// What a `setLayer` call names, resolved when it was tried: a layer, a song by its first piece, or a clip, and how the
/// model named it. A call that waits for the removals is made again from it on that phase's copy (`again`).
#[derive(Clone, Debug)]
pub(crate) enum Target {
    Layer(Id, String),
    Song(String, String),
    Clip(Id, String),
}

/// `setLayer`'s edit on `target` of `work`, `change` as the call gave it.
pub(super) fn again(
    c: &Compiler,
    work: &Project,
    target: &Target,
    change: &LayerChange,
) -> Result<Edit, Refused> {
    match target {
        Target::Layer(id, said) => set_layer(c, work, (id, said), change),
        Target::Song(first, said) => set_song_sound(work, (first, said), change),
        Target::Clip(id, said) => set_clip_fades(work, (id, said), change),
    }
}
