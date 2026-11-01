//! Volume keys: a clip's or sound layer's level over time, set by hand. Each key is a gain (0 to 2, as `volume` is)
//! at a time from the clip's or layer's start, with the ease it moves into it by, as a layer's motion keys are
//! (`motion::Key`). With keys the line is the level, and the volume is what it plays at once they are gone. Edits move
//! and trim them with their clip or layer as they do motion keys: they keep their place on the content, and where an
//! edit cuts the line a key holds the level there.

use crate::motion::{self, Key, MAX_KEYS};
use crate::time::{scaled, unscaled, Us};

/// The levels a key may hold: silent to twice as loud as recorded, as the volume.
pub const LIMITS: (f64, f64) = (0.0, 2.0);

/// What the helpers below hold a cut's value from when there are no keys: never read, as they do nothing then.
const REST: f64 = 1.0;

/// The level at `t_us` from the start: the line's where there are keys (the first key's before it, the last's after
/// it), else `volume`. An ease that overshoots stays within [`LIMITS`]. No allocation (the mixer calls it per sample).
pub fn level_at(keys: &[Key], t_us: Us, volume: f64) -> f64 {
    if keys.is_empty() {
        return volume;
    }
    motion::value_at(keys, t_us, volume).clamp(LIMITS.0, LIMITS.1)
}

/// Whether nothing is heard at any time: every key silent, or no keys and no volume.
pub fn silent(keys: &[Key], volume: f64) -> bool {
    if keys.is_empty() {
        volume <= 0.0
    } else {
        keys.iter().all(|k| k.v <= 0.0)
    }
}

/// The two pieces of a line `len_us` long split `at_us` into it: each keeps the keys on its side, with one at the cut
/// holding the level there when keys went.
pub fn split(keys: &[Key], at_us: Us) -> (Vec<Key>, Vec<Key>) {
    let (mut first, mut second) = (keys.to_vec(), keys.to_vec());
    motion::cut_end(&mut first, at_us, REST, LIMITS);
    motion::cut_start(&mut second, at_us, REST, LIMITS);
    (first, second)
}

/// The clip or layer lost `by_us` at its start: keys keep their place on the content, and those at or before the new
/// start become one key at 0 holding the level there.
pub fn trim_start(keys: &mut Vec<Key>, by_us: Us) {
    motion::cut_start(keys, by_us, REST, LIMITS);
}

/// The clip or layer grew `by_us` at its start: keys keep their place on the content, later by as much.
pub fn extend_start(keys: &mut [Key], by_us: Us) {
    keys.iter_mut()
        .for_each(|k| k.t_us = k.t_us.saturating_add(by_us));
}

/// The clip or layer is now `len_us` long: keys past the end go, and one at the end holds the level there.
pub fn fit(keys: &mut Vec<Key>, len_us: Us) {
    motion::cut_end(keys, len_us, REST, LIMITS);
}

/// The part of the line from `from_us`, `len_us` long, for a piece of the clip or layer a cut kept.
pub fn piece(keys: &[Key], from_us: Us, len_us: Us) -> Vec<Key> {
    let mut part = keys.to_vec();
    trim_start(&mut part, from_us);
    fit(&mut part, len_us);
    part
}

/// The footage now plays at `new_speed` instead of `old_speed`, and the clip is `len_us` long: keys rescale so they
/// stay on the same footage.
pub fn rescale(keys: &mut Vec<Key>, old_speed: f64, new_speed: f64, len_us: Us) {
    keys.iter_mut()
        .for_each(|k| k.t_us = scaled(unscaled(k.t_us, old_speed), new_speed));
    keys.dedup_by(|later, earlier| later.t_us <= earlier.t_us);
    fit(keys, len_us);
}

/// Each key's level times `gain`, within [`LIMITS`]: the line of a sound played at `gain` of its clip's level (a
/// separated source at its mix's volume).
pub fn times(keys: &[Key], gain: f64) -> Vec<Key> {
    keys.iter()
        .map(|k| Key {
            v: (k.v * gain).clamp(LIMITS.0, LIMITS.1),
            ..k.clone()
        })
        .collect()
}

/// The keys of a clip or layer whose whole volume went from `old` to `new` (a slider, chat's "turn it down"): each key
/// times `new / old`, within [`LIMITS`], so the line moves with the volume; from no volume, every key takes `new`.
pub fn rescaled(keys: &[Key], old: f64, new: f64) -> Vec<Key> {
    if old > 0.0 {
        times(keys, new / old)
    } else {
        keys.iter()
            .map(|k| Key {
                v: new.clamp(LIMITS.0, LIMITS.1),
                ..k.clone()
            })
            .collect()
    }
}

/// Why validation refuses the keys of a clip or layer `len_us` long, as a model code, or None.
pub(crate) fn problem(keys: &[Key], len_us: Us) -> Option<&'static str> {
    if keys.len() > MAX_KEYS {
        return Some(crate::messages::code!("model.volume.tooManyKeys"));
    }
    if keys.iter().any(|k| k.ease.is_some_and(|e| !e.is_valid())) {
        return Some(crate::messages::code!("model.volume.badEase"));
    }
    let (lo, hi) = LIMITS;
    if keys
        .iter()
        .any(|k| !(k.v.is_finite() && k.v >= lo && k.v <= hi))
    {
        return Some(crate::messages::code!("model.volume.badValue"));
    }
    if keys.windows(2).any(|w| w[0].t_us >= w[1].t_us) {
        return Some(crate::messages::code!("model.volume.unsorted"));
    }
    if keys.iter().any(|k| k.t_us < 0 || k.t_us > len_us) {
        return Some(crate::messages::code!("model.volume.outside"));
    }
    None
}
