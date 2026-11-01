//! `setMotion` on a layer of the additions' working copy, or on a sticker still to be drawn: its keys and presets as
//! the model stores them (`motion::Motion`), so the engine draws them in the preview and the export alike and the
//! layer's card shows them.
//! - Each property listed gets exactly the keys given, in time order; the others keep theirs; an empty list takes every
//!   key away. A key past the layer's end, two keys of a property at one time, or more than `motion::MAX_KEYS` are
//!   refused back to the model with what to change.
//! - A preset is the model's built-in of that word (`slide` with its side, a loop with its strength), lasting what was
//!   asked or as `animate`'s do (`animate::ENTER_US`, `EXIT_US`, `EMPHASIS_US`), cut to the layer's length; `none`
//!   takes its slot's away. A typewriter is for words.

use std::collections::BTreeMap;

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::contrib::builtin;
use klipolo_model::ids::Id;
use klipolo_model::motion::{self, Key, MAX_KEYS, Motion, MotionKeys, MotionUse};
use klipolo_model::ops::{ClipPatch, ItemPatch, Op};
use klipolo_model::params::ParamValue;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Item, ItemContent};

use super::animate::{EMPHASIS_STRENGTH, EMPHASIS_US, ENTER_US, EXIT_US, noun};
use super::{Edit, Refused, label, to_model};
use crate::context::secs;
use crate::tools::{KeyIn, PresetIn, Property, Slot};

/// What a layer shows, for the typewriter: words, animated words (which come in by their own units), or anything else.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shows {
    Words,
    AnimatedWords,
    Other,
}

/// `now` with `keys` and `presets`, for a layer `len_us` long that `shows`: what `setMotion` leaves, or why it can't,
/// for the model.
pub(crate) fn moved(
    now: Option<Motion>,
    keys: Option<&[KeyIn]>,
    presets: &[PresetIn],
    len_us: Us,
    shows: Shows,
) -> Result<Option<Motion>, String> {
    let mut m = now.unwrap_or_default();
    if let Some(keys) = keys {
        m.keys = keyed(m.keys.take(), keys, len_us)?;
    }
    for p in presets {
        match (p.preset, shows) {
            (Some("typewriter"), Shows::Other) => {
                return Err(
                    "setMotion: presets: typewriter is for words. Use another preset for this layer.".to_owned(),
                );
            }
            (Some("typewriter"), Shows::AnimatedWords) => {
                return Err("setMotion: presets: animated words come in by their own units. Change how with \
                            animateText."
                    .to_owned());
            }
            _ => {}
        }
        let used = p.preset.map(|word| {
            let mut params = BTreeMap::new();
            if let Some(from) = p.from {
                params.insert("from".to_owned(), ParamValue::Text(from.to_owned()));
            }
            if p.slot == Slot::Loop {
                let strength = p.strength.unwrap_or(EMPHASIS_STRENGTH);
                params.insert("strength".to_owned(), ParamValue::Number(strength));
            }
            let default = match p.slot {
                Slot::Enter => ENTER_US,
                Slot::Exit => EXIT_US,
                Slot::Loop => EMPHASIS_US,
            };
            MotionUse {
                preset: builtin::motion(word),
                params,
                duration_us: p.duration_us.unwrap_or(default),
                ease: p.ease,
            }
        });
        match p.slot {
            Slot::Enter => m.enter = used,
            Slot::Exit => m.exit = used,
            Slot::Loop => m.looping = used,
        }
    }
    m.fit(len_us);
    Ok(motion::kept(m))
}

/// `now`'s keys with each property `keys` lists set to exactly its keys; an empty `keys` takes them all away.
fn keyed(
    now: Option<MotionKeys>,
    keys: &[KeyIn],
    len_us: Us,
) -> Result<Option<MotionKeys>, String> {
    if keys.is_empty() {
        return Ok(None);
    }
    let mut out = now.unwrap_or_default();
    let mut given: BTreeMap<Property, Vec<(usize, &KeyIn)>> = BTreeMap::new();
    for (i, k) in keys.iter().enumerate() {
        if k.at_us > len_us {
            return Err(format!(
                "setMotion: keys[{i}].atSec {} is past the layer's end; it lasts {} s, so use 0 to {}.",
                secs(k.at_us),
                secs(len_us),
                secs(len_us)
            ));
        }
        given.entry(k.property).or_default().push((i, k));
    }
    for (property, mut list) in given {
        list.sort_by_key(|(_, k)| k.at_us);
        if let Some(w) = list.windows(2).find(|w| w[0].1.at_us == w[1].1.at_us) {
            return Err(format!(
                "setMotion: keys[{}] and keys[{}] both set {} at {} s; give each time one value.",
                w[0].0.min(w[1].0),
                w[0].0.max(w[1].0),
                word(property),
                secs(w[0].1.at_us)
            ));
        }
        if list.len() > MAX_KEYS {
            return Err(format!(
                "setMotion: {} has {} keys; a property has at most {MAX_KEYS}.",
                word(property),
                list.len()
            ));
        }
        let set: Vec<Key> = (list.into_iter())
            .map(|(_, k)| Key {
                t_us: k.at_us,
                v: k.value,
                ease: k.ease,
            })
            .collect();
        let slot = match property {
            Property::X => &mut out.x,
            Property::Y => &mut out.y,
            Property::Scale => &mut out.scale,
            Property::Rotation => &mut out.rotation,
            Property::Opacity => &mut out.opacity,
        };
        *slot = set;
    }
    Ok((!out.is_empty()).then_some(out))
}

fn word(p: Property) -> &'static str {
    match p {
        Property::X => "x",
        Property::Y => "y",
        Property::Scale => "scale",
        Property::Rotation => "rotation",
        Property::Opacity => "opacity",
    }
}

/// The layer `id` of `work`, named `said` for `tool`: one that draws.
pub(super) fn drawn<'p>(
    work: &'p Project,
    (id, said): (&Id, &str),
    tool: &str,
) -> Result<&'p Item, Refused> {
    let Some(item) = (work.timeline.tracks.iter())
        .flat_map(|t| &t.items)
        .find(|i| &i.id == id)
    else {
        return Err(to_model(format!("{tool}: {said} is gone.")));
    };
    if !item.content.is_visual() || matches!(item.content, ItemContent::Adjustment) {
        return Err(to_model(format!("{tool}: {said} draws nothing to change.")));
    }
    Ok(item)
}

/// `setMotion` on the layer `id` of `work`: its new motion as one `UpdateItem`.
pub(super) fn set_motion(
    work: &Project,
    target: (&Id, &str),
    keys: Option<&[KeyIn]>,
    presets: &[PresetIn],
) -> Result<Edit, Refused> {
    let item = drawn(work, target, "setMotion")?;
    let shows = match &item.content {
        ItemContent::Text { .. } => Shows::Words,
        ItemContent::Generator { source, .. }
            if source.as_str() == klipolo_model::media::text_motion::SOURCE =>
        {
            Shows::AnimatedWords
        }
        _ => Shows::Other,
    };
    let motion = moved(
        item.layout.motion.clone(),
        keys,
        presets,
        item.duration_us,
        shows,
    )
    .map_err(to_model)?;
    let mut layout = item.layout.clone();
    layout.motion = motion;
    let ops = if layout == item.layout {
        Vec::new()
    } else {
        vec![Op::UpdateItem {
            id: item.id.clone(),
            patch: ItemPatch {
                layout: Some(layout),
                ..ItemPatch::default()
            },
        }]
    };
    Ok(Edit {
        ops,
        label: label("setMotion", vec![("layer", ErrorParam::from(noun(item)))]),
    })
}

/// `setMotion` on the main track's clip `id`, named `said`: the keys of its recording's camera bubble, which moves and
/// scales (x, y, scale) but takes no presets. Keys come from the clip's start, in its timeline time, and are kept in
/// the recording's own time, as the bubble's keys are (`CameraLayout::bubble`), so cuts and speed changes keep them on
/// the content. Each property listed gets exactly these keys; an empty list takes them all away.
pub(super) fn bubble_motion(
    work: &Project,
    (id, said): (&Id, &str),
    keys: Option<&[KeyIn]>,
    presets: &[PresetIn],
) -> Result<Edit, Refused> {
    let Some(clip) = work.timeline.main.iter().find(|c| &c.id == id) else {
        return Err(to_model(format!("setMotion: {said} is gone.")));
    };
    let Some(camera) = &clip.camera else {
        return Err(to_model(format!(
            "setMotion: {said} has no camera bubble. setMotion moves layers (t1, i1, v1) and a recording's camera."
        )));
    };
    if !presets.is_empty() {
        return Err(to_model(format!(
            "setMotion: the camera bubble takes keys only (x, y, scale), no presets. Leave presets null for {said}."
        )));
    }
    let keys = keys.unwrap_or_default();
    if let Some(k) = keys
        .iter()
        .find(|k| matches!(k.property, Property::Rotation | Property::Opacity))
    {
        return Err(to_model(format!(
            "setMotion: the camera bubble takes x, y and scale keys, not {}.",
            word(k.property)
        )));
    }
    let mut motion = camera.bubble.motion.clone().unwrap_or_default();
    // Checked in the clip's own time, then kept in the recording's.
    let timed = keyed(motion.keys.take(), keys, clip.duration_us()).map_err(to_model)?;
    let source = |t: Us| {
        clip.source_in_us
            .saturating_add(klipolo_model::time::unscaled(t, clip.speed))
    };
    motion.keys = timed.map(|mut k| {
        for (list, property) in [
            (&mut k.x, Property::X),
            (&mut k.y, Property::Y),
            (&mut k.scale, Property::Scale),
        ] {
            if keys.iter().any(|g| g.property == property) {
                list.iter_mut().for_each(|key| key.t_us = source(key.t_us));
            }
        }
        k
    });
    let mut camera = camera.clone();
    camera.bubble.motion = motion::kept(motion);
    let ops = if Some(&camera) == clip.camera.as_ref() {
        Vec::new()
    } else {
        vec![Op::UpdateClip {
            id: id.clone(),
            patch: ClipPatch {
                camera: Some(camera),
                ..ClipPatch::default()
            },
        }]
    };
    Ok(Edit {
        ops,
        label: label("setMotion", vec![("layer", ErrorParam::from("camera"))]),
    })
}
