//! A node's own keys (`animate`) as tracks, and its modifiers.

use arrayvec::ArrayVec;

use super::super::eval::tracks::us;
use super::super::timeline::{lower_of, CompiledScene, Lower, ModifierFn, Prop, Stagger, Track};
use super::super::{SceneKey, SceneLimits, Unit};
use super::lower::{holds, Sink};
use super::statics::ease;
use crate::motion::{Ease, Key};

/// Compiled node `c`'s modifiers, each with whether its seed was left to Klipolo (each instance then wiggles its own).
pub(crate) fn modifiers(cs: &CompiledScene, c: usize, seed: u32) -> Vec<(Track, bool)> {
    let Some(cn) = cs.nodes.get(c) else {
        return Vec::new();
    };
    let mut mods = Vec::new();
    for (name, m) in cn.node.props.modifiers.iter().flatten() {
        let prop = match lower_of(name) {
            Some(Lower::Num(p)) | Some(Lower::Pos(p, _)) => p,
            _ => continue,
        };
        let reseed = matches!(m, super::super::Modifier::Wiggle { seed: None, .. });
        let track = Track {
            prop,
            keys: Vec::new(),
            stagger: None,
            repeat: None,
            modifier: Some(ModifierFn::of(
                m,
                seed ^ (prop as u32).wrapping_mul(0x9e37_79b9),
            )),
            unit: false,
            tween: None,
            chain: None,
            placed_us: 0,
        };
        mods.push((track, reseed));
    }
    mods
}

/// A node's own keys for one property: a track per number it writes, linear unless a key names an ease; on a split
/// text, a property units have runs per unit, offset by the text's own stagger.
pub(super) fn own_keys(
    sink: &mut Sink,
    lower: Lower,
    keys: &[SceneKey],
    split: bool,
    stagger: Option<Stagger>,
    tracks: &mut Vec<Track>,
) {
    let mut made: ArrayVec<(Prop, Vec<Key>), 4> = ArrayVec::new();
    for key in keys.iter().take(SceneLimits::MAX_KEYS) {
        if !key.t.is_finite() {
            continue;
        }
        let Some(values) = sink.values(lower, &key.v, key.unit == Some(Unit::Px)) else {
            continue;
        };
        for (prop, v) in values {
            let e = if holds(prop) {
                Ease::Hold
            } else {
                ease(key.ease.as_deref(), Ease::Linear)
            };
            let k = Key {
                t_us: us(key.t),
                v,
                ease: Some(e),
            };
            match made.iter_mut().find(|(p, _)| *p == prop) {
                Some((_, list)) => list.push(k),
                None => {
                    if made.try_push((prop, vec![k])).is_err() {
                        break;
                    }
                }
            }
        }
    }
    for (prop, mut list) in made {
        // In time order, as the check requires (an unchecked scene's keys are sorted, not refused).
        list.sort_by_key(|k| k.t_us);
        // A text or on/off switches at its first key: before it the node shows its own.
        let chain = match list.first() {
            Some(first) if holds(prop) => {
                // A microsecond before, so the key's own value shows from its time on.
                let at = first.t_us - 1;
                list.insert(
                    0,
                    Key {
                        t_us: at,
                        v: 0.0,
                        ease: None,
                    },
                );
                Some(0)
            }
            _ => None,
        };
        let unit = split && prop.per_unit();
        tracks.push(Track {
            prop,
            keys: list,
            stagger: if unit { stagger } else { None },
            repeat: None,
            modifier: None,
            unit,
            tween: None,
            chain,
            placed_us: 0,
        });
    }
}
