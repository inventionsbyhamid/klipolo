//! Layer motion: keyframed position, scale, rotation and
//! opacity with eases on every visual layer and on the camera bubble, plus enter, exit and loop presets. One key and
//! ease type and one evaluator, pure and allocation-free, shared later by `scene@1`. Presets are contributions
//! (`ContribKind::Motion`) that the engine evaluates per format; this module owns the keys, the eases and
//! what edits do to them.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::contrib::ContribId;
use crate::params::ParamValue;
use crate::time::{scaled, unscaled, Us, MAX_TIME_US};

pub(crate) mod reach;
pub(crate) mod spring;

pub use spring::{spring_peak, MIN_SPRING_ZETA};

/// At most this many keys per property (validation).
pub const MAX_KEYS: usize = 64;

/// The values each property allows, in `MotionKeys::each` order (x, y, scale, rotation, opacity): validation refuses a
/// key outside them, and a key an edit adds to hold a value is clamped to them (an eased overshoot can pass them).
pub const LIMITS: [(f64, f64); 5] = [
    (-4.0, 4.0),
    (-4.0, 4.0),
    (0.0, 20.0),
    (-3600.0, 3600.0),
    (0.0, 1.0),
];

/// The values a cubic Bézier ease's `y1` and `y2` may take (validation): wide enough for any real
/// overshoot (CSS's back eases stay within -0.6..1.6), and narrow enough that a crafted ease can't make a layer reach
/// an absurd size.
pub const EASE_Y: (f64, f64) = (-2.0, 3.0);

/// The most jumps a `steps(n)` ease makes (validation).
pub const MAX_STEPS: u32 = 64;

/// How a value moves into a key: CSS's and GSAP's names, implemented from their published maths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Ease {
    #[default]
    Linear,
    /// CSS `ease`, `ease-in`, `ease-out` and `ease-in-out`.
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
    /// GSAP `power1`…`power4` (quad, cubic, quart, quint).
    Power1In,
    Power1Out,
    Power1InOut,
    Power2In,
    Power2Out,
    Power2InOut,
    Power3In,
    Power3Out,
    Power3InOut,
    Power4In,
    Power4Out,
    Power4InOut,
    BackIn,
    BackOut,
    BackInOut,
    ElasticIn,
    ElasticOut,
    ElasticInOut,
    BounceIn,
    BounceOut,
    BounceInOut,
    /// CSS `cubic-bezier(x1, y1, x2, y2)`; x1 and x2 in 0–1, y1 and y2 in [`EASE_Y`].
    CubicBezier {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
    /// GSAP `sine`, `expo` and `circ`.
    SineIn,
    SineOut,
    SineInOut,
    ExpoIn,
    ExpoOut,
    ExpoInOut,
    CircIn,
    CircOut,
    CircInOut,
    /// CSS `steps(n)` (`jump-end`): n equal jumps, each at the end of its step; n in 1–[`MAX_STEPS`].
    Steps {
        n: u32,
    },
    /// A damped spring released from rest, stretched so the key's span is its settle time: stiffness 1–1000,
    /// damping 0–100, mass 0.1–10, and a damping ratio of at least [`MIN_SPRING_ZETA`] (`spring` module).
    Spring {
        stiffness: f64,
        damping: f64,
        mass: f64,
    },
    /// The earlier value until the key, then the key's.
    Hold,
}

const C1: f64 = 1.701_58;
const C2: f64 = C1 * 1.525;
const C3: f64 = C1 + 1.0;
const C4: f64 = 2.0 * std::f64::consts::PI / 3.0;
const C5: f64 = 2.0 * std::f64::consts::PI / 4.5;

fn power_in(n: i32, t: f64) -> f64 {
    t.powi(n)
}

fn power_out(n: i32, t: f64) -> f64 {
    1.0 - (1.0 - t).powi(n)
}

fn power_in_out(n: i32, t: f64) -> f64 {
    if t < 0.5 {
        (2.0 * t).powi(n) / 2.0
    } else {
        1.0 - (2.0 - 2.0 * t).powi(n) / 2.0
    }
}

fn bounce_out(t: f64) -> f64 {
    const N1: f64 = 7.5625;
    const D1: f64 = 2.75;
    if t < 1.0 / D1 {
        N1 * t * t
    } else if t < 2.0 / D1 {
        let t = t - 1.5 / D1;
        N1 * t * t + 0.75
    } else if t < 2.5 / D1 {
        let t = t - 2.25 / D1;
        N1 * t * t + 0.9375
    } else {
        let t = t - 2.625 / D1;
        N1 * t * t + 0.984_375
    }
}

/// `cubic-bezier(x1, y1, x2, y2)` at `t`: Newton's method on x, then bisection if it stalls.
fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let curve = |a: f64, b: f64, s: f64| {
        3.0 * a * s * (1.0 - s).powi(2) + 3.0 * b * s * s * (1.0 - s) + s * s * s
    };
    let slope = |a: f64, b: f64, s: f64| {
        3.0 * a * (1.0 - s).powi(2) + 6.0 * (b - a) * s * (1.0 - s) + 3.0 * (1.0 - b) * s * s
    };
    let mut s = t;
    for _ in 0..8 {
        let err = curve(x1, x2, s) - t;
        if err.abs() < 1e-7 {
            return curve(y1, y2, s);
        }
        let d = slope(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - err / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    s = t;
    for _ in 0..30 {
        let x = curve(x1, x2, s);
        if (x - t).abs() < 1e-7 {
            break;
        }
        if x < t {
            lo = s
        } else {
            hi = s
        }
        s = (lo + hi) / 2.0;
    }
    curve(y1, y2, s)
}

impl Ease {
    /// Progress `t` (0–1) through the ease: 0 → 0 and 1 → 1; back, elastic and spring overshoot in between.
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::Ease => bezier(0.25, 0.1, 0.25, 1.0, t),
            Self::EaseIn => bezier(0.42, 0.0, 1.0, 1.0, t),
            Self::EaseOut => bezier(0.0, 0.0, 0.58, 1.0, t),
            Self::EaseInOut => bezier(0.42, 0.0, 0.58, 1.0, t),
            Self::Power1In => power_in(2, t),
            Self::Power1Out => power_out(2, t),
            Self::Power1InOut => power_in_out(2, t),
            Self::Power2In => power_in(3, t),
            Self::Power2Out => power_out(3, t),
            Self::Power2InOut => power_in_out(3, t),
            Self::Power3In => power_in(4, t),
            Self::Power3Out => power_out(4, t),
            Self::Power3InOut => power_in_out(4, t),
            Self::Power4In => power_in(5, t),
            Self::Power4Out => power_out(5, t),
            Self::Power4InOut => power_in_out(5, t),
            Self::BackIn => C3 * t * t * t - C1 * t * t,
            Self::BackOut => 1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2),
            Self::BackInOut if t < 0.5 => (2.0 * t).powi(2) * ((C2 + 1.0) * 2.0 * t - C2) / 2.0,
            Self::BackInOut => {
                ((2.0 * t - 2.0).powi(2) * ((C2 + 1.0) * (2.0 * t - 2.0) + C2) + 2.0) / 2.0
            }
            Self::ElasticIn | Self::ElasticOut | Self::ElasticInOut if t == 0.0 || t == 1.0 => t,
            Self::ElasticIn => -(2f64.powf(10.0 * t - 10.0)) * ((10.0 * t - 10.75) * C4).sin(),
            Self::ElasticOut => 2f64.powf(-10.0 * t) * ((10.0 * t - 0.75) * C4).sin() + 1.0,
            Self::ElasticInOut if t < 0.5 => {
                -(2f64.powf(20.0 * t - 10.0) * ((20.0 * t - 11.125) * C5).sin()) / 2.0
            }
            Self::ElasticInOut => {
                2f64.powf(-20.0 * t + 10.0) * ((20.0 * t - 11.125) * C5).sin() / 2.0 + 1.0
            }
            Self::BounceIn => 1.0 - bounce_out(1.0 - t),
            Self::BounceOut => bounce_out(t),
            Self::BounceInOut if t < 0.5 => (1.0 - bounce_out(1.0 - 2.0 * t)) / 2.0,
            Self::BounceInOut => (1.0 + bounce_out(2.0 * t - 1.0)) / 2.0,
            Self::CubicBezier { x1, y1, x2, y2 } => bezier(x1, y1, x2, y2, t),
            // cos(π/2) isn't quite 0 in f64: pinned, so the key's value is reached exactly.
            Self::SineIn if t == 1.0 => 1.0,
            Self::SineIn => 1.0 - (t * std::f64::consts::FRAC_PI_2).cos(),
            Self::SineOut => (t * std::f64::consts::FRAC_PI_2).sin(),
            Self::SineInOut => -((std::f64::consts::PI * t).cos() - 1.0) / 2.0,
            Self::ExpoIn if t == 0.0 => 0.0,
            Self::ExpoIn => 2f64.powf(10.0 * t - 10.0),
            Self::ExpoOut if t == 1.0 => 1.0,
            Self::ExpoOut => 1.0 - 2f64.powf(-10.0 * t),
            Self::ExpoInOut if t == 0.0 || t == 1.0 => t,
            Self::ExpoInOut if t < 0.5 => 2f64.powf(20.0 * t - 10.0) / 2.0,
            Self::ExpoInOut => (2.0 - 2f64.powf(-20.0 * t + 10.0)) / 2.0,
            Self::CircIn => 1.0 - (1.0 - t * t).sqrt(),
            Self::CircOut => (1.0 - (t - 1.0).powi(2)).sqrt(),
            Self::CircInOut if t < 0.5 => (1.0 - (1.0 - (2.0 * t).powi(2)).sqrt()) / 2.0,
            Self::CircInOut => ((1.0 - (-2.0 * t + 2.0).powi(2)).sqrt() + 1.0) / 2.0,
            Self::Steps { .. } | Self::Hold if t >= 1.0 => 1.0,
            Self::Steps { n } => (t * f64::from(n.max(1))).floor() / f64::from(n.max(1)),
            Self::Hold => 0.0,
            Self::Spring {
                stiffness,
                damping,
                mass,
            } => spring::spring(stiffness, damping, mass, t),
        }
    }

    /// Whether validation lets the ease in: a cubic Bézier within its bounds, 1–[`MAX_STEPS`] steps, and a spring
    /// in range and damped enough (a crafted ease can't send a layer to an absurd size).
    pub fn is_valid(self) -> bool {
        match self {
            Self::CubicBezier { x1, y1, x2, y2 } => {
                let y = EASE_Y.0..=EASE_Y.1;
                (0.0..=1.0).contains(&x1)
                    && (0.0..=1.0).contains(&x2)
                    && y.contains(&y1)
                    && y.contains(&y2)
            }
            Self::Steps { n } => (1..=MAX_STEPS).contains(&n),
            Self::Spring {
                stiffness,
                damping,
                mass,
            } => spring::is_valid(stiffness, damping, mass),
            _ => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Key {
    /// From the layer's start; a camera bubble's keys are in its recording's source time, so cuts and ripples keep
    /// them on the content.
    pub t_us: Us,
    pub v: f64,
    /// How the value moves into this key from the one before (linear when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ease: Option<Ease>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export)]
pub struct MotionKeys {
    /// Offsets of the box, in canvas fractions of the primary format (other formats apply the same fractions).
    #[serde(default)]
    pub x: Vec<Key>,
    #[serde(default)]
    pub y: Vec<Key>,
    /// About the box's centre; 1 is its size.
    #[serde(default)]
    pub scale: Vec<Key>,
    /// Degrees, added to `Layout.rotation`.
    #[serde(default)]
    pub rotation: Vec<Key>,
    /// Multiplies `Layout.opacity`.
    #[serde(default)]
    pub opacity: Vec<Key>,
}

impl MotionKeys {
    pub fn is_empty(&self) -> bool {
        self.x.is_empty()
            && self.y.is_empty()
            && self.scale.is_empty()
            && self.rotation.is_empty()
            && self.opacity.is_empty()
    }

    /// Each property with its rest value (0, 0, 1, 0, 1).
    pub fn each_mut(&mut self) -> [(&mut Vec<Key>, f64); 5] {
        [
            (&mut self.x, 0.0),
            (&mut self.y, 0.0),
            (&mut self.scale, 1.0),
            (&mut self.rotation, 0.0),
            (&mut self.opacity, 1.0),
        ]
    }

    pub fn each(&self) -> [(&Vec<Key>, f64); 5] {
        [
            (&self.x, 0.0),
            (&self.y, 0.0),
            (&self.scale, 1.0),
            (&self.rotation, 0.0),
            (&self.opacity, 1.0),
        ]
    }
}

/// A preset in use: `klipolo.builtin/motion.slide` with `{ from: "top" }` for 0.5 s.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MotionUse {
    /// A preset this build doesn't know is kept and does nothing, with a note naming it.
    pub preset: ContribId,
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,
    /// How long the enter or exit takes, or one cycle of a loop; at most the layer's length.
    pub duration_us: Us,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ease: Option<Ease>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Motion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub enter: Option<MotionUse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub exit: Option<MotionUse>,
    #[serde(rename = "loop", default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub looping: Option<MotionUse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub keys: Option<MotionKeys>,
}

/// How a layer mixes with what is under it. Normal, multiply, screen and add are blend states; overlay reads the
/// frame below, like an adjustment layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Add,
    Overlay,
}

/// What the keys say at one instant, applied to the static layout: offsets, a scale about the centre, degrees added,
/// and an opacity factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub dx: f64,
    pub dy: f64,
    pub scale: f64,
    pub rotation: f64,
    pub opacity: f64,
}

impl Sample {
    pub const REST: Self = Self {
        dx: 0.0,
        dy: 0.0,
        scale: 1.0,
        rotation: 0.0,
        opacity: 1.0,
    };
}

/// One property at `t_us`: the first key's value before it, the last key's after it, eased in between; `rest` with
/// no keys. No allocation (the engine calls it every frame).
pub fn value_at(keys: &[Key], t_us: Us, rest: f64) -> f64 {
    let Some(first) = keys.first() else {
        return rest;
    };
    if t_us <= first.t_us {
        return first.v;
    }
    let next = keys.partition_point(|k| k.t_us <= t_us);
    let Some(b) = keys.get(next) else {
        return keys[keys.len() - 1].v;
    };
    let a = &keys[next - 1];
    let span = (b.t_us - a.t_us).max(1) as f64;
    a.v + (b.v - a.v)
        * b.ease
            .unwrap_or_default()
            .apply((t_us - a.t_us) as f64 / span)
}

/// The keyed part of `m` at `t_us` into a layer `len_us` long (presets are the engine's).
pub fn sample(m: &Motion, t_us: Us, len_us: Us) -> Sample {
    let Some(k) = &m.keys else {
        return Sample::REST;
    };
    let t = t_us.clamp(0, len_us.max(0));
    Sample {
        dx: value_at(&k.x, t, 0.0),
        dy: value_at(&k.y, t, 0.0),
        scale: value_at(&k.scale, t, 1.0),
        rotation: value_at(&k.rotation, t, 0.0),
        opacity: value_at(&k.opacity, t, 1.0),
    }
}

/// The largest scale the keys reach (at least 1), counting an ease's overshoot between two keys (`reach`): an effect
/// target is sized for it.
pub fn max_key_scale(m: &Motion) -> f64 {
    m.keys
        .as_ref()
        .map_or(1.0, |k| reach::keys_max(&k.scale, 1.0).max(1.0))
}

/// A key at `t_us` holding the value there, within the property's `limits`.
fn held(keys: &[Key], t_us: Us, rest: f64, (lo, hi): (f64, f64)) -> Key {
    Key {
        t_us,
        v: value_at(keys, t_us, rest).clamp(lo, hi),
        ease: None,
    }
}

/// One property of a layer now ending at `len_us`: keys past the end go, and when any did, a key at the end holds the
/// value there, so what is kept moves as it did.
fn cut_end(keys: &mut Vec<Key>, len_us: Us, rest: f64, limits: (f64, f64)) {
    if keys.last().is_none_or(|k| k.t_us <= len_us) {
        return;
    }
    let end = held(keys, len_us, rest, limits);
    keys.retain(|k| k.t_us <= len_us);
    if keys.last().is_none_or(|k| k.t_us < len_us) {
        keys.push(end);
    }
}

/// One property of a layer that lost `by_us` at its start: keys keep their place on the content, and when any was at
/// or before the new start, one key at 0 holds the value there (keys all after it already hold their first value).
fn cut_start(keys: &mut Vec<Key>, by_us: Us, rest: f64, limits: (f64, f64)) {
    if by_us <= 0 || keys.is_empty() {
        return;
    }
    let dropped = keys[0].t_us <= by_us;
    let start = Key {
        t_us: 0,
        ..held(keys, by_us, rest, limits)
    };
    keys.retain(|k| k.t_us > by_us);
    keys.iter_mut().for_each(|k| k.t_us -= by_us);
    if dropped {
        keys.insert(0, start);
    }
}

/// One property of a layer `old_len_us` long whose sorted, disjoint `cuts` (in its time) were taken out: the keys
/// left keep their place on the content, and where a cut took keys away, keys at the join hold the values on each
/// side of it (1 µs apart). Without room for those (`MAX_KEYS`), the keys left are only moved.
fn cut_ranges(
    keys: &mut Vec<Key>,
    cuts: &[(Us, Us)],
    old_len_us: Us,
    rest: f64,
    limits: (f64, f64),
) {
    if keys.is_empty() || cuts.is_empty() {
        return;
    }
    // Time cut away before each cut, so a key's new place is a binary search away.
    let mut before = Vec::with_capacity(cuts.len());
    let mut total: Us = 0;
    for &(a, b) in cuts {
        before.push(total);
        total = total.saturating_add(b - a);
    }
    let original = std::mem::take(keys);
    let mut emptied = Vec::new();
    for k in &original {
        let i = cuts.partition_point(|&(_, b)| b <= k.t_us);
        if cuts.get(i).is_some_and(|&(a, _)| a <= k.t_us) {
            if emptied.last() != Some(&i) {
                emptied.push(i);
            }
        } else {
            let gone = before.get(i).copied().unwrap_or(total);
            keys.push(Key {
                t_us: k.t_us - gone,
                ..k.clone()
            });
        }
    }
    let moved = keys.len();
    for &i in &emptied {
        let (a, b) = cuts[i];
        let join = a - before[i];
        let (lo, hi) = limits;
        let left = value_at(&original, a, rest).clamp(lo, hi);
        let right = value_at(&original, b, rest).clamp(lo, hi);
        if a > 0 {
            keys.push(Key {
                t_us: join - 1,
                v: left,
                ease: None,
            });
        }
        if b < old_len_us && (a == 0 || right != left) {
            keys.push(Key {
                t_us: join,
                v: right,
                ease: None,
            });
        }
    }
    if keys.len() > MAX_KEYS {
        keys.truncate(moved);
    }
    keys.sort_by_key(|k| k.t_us);
    keys.dedup_by(|later, earlier| later.t_us <= earlier.t_us);
}

impl Motion {
    pub fn is_empty(&self) -> bool {
        self.enter.is_none()
            && self.exit.is_none()
            && self.looping.is_none()
            && self.keys.as_ref().is_none_or(MotionKeys::is_empty)
    }

    /// The two pieces of a layer `len_us` long split `at_us` into it: each keeps the keys on its side, plus one at the
    /// cut holding the value there when keys went; enter stays on the first piece, exit goes to the second, loop to
    /// both.
    pub fn split(&self, at_us: Us, len_us: Us) -> (Motion, Motion) {
        let mut first = Motion {
            exit: None,
            ..self.clone()
        };
        let mut second = Motion {
            enter: None,
            ..self.clone()
        };
        if let (Some(a), Some(b)) = (first.keys.as_mut(), second.keys.as_mut()) {
            for ((keys, rest), limits) in a.each_mut().into_iter().zip(LIMITS) {
                cut_end(keys, at_us, rest, limits);
            }
            for ((keys, rest), limits) in b.each_mut().into_iter().zip(LIMITS) {
                cut_start(keys, at_us, rest, limits);
            }
        }
        first.fit(at_us);
        second.fit(len_us - at_us);
        (first, second)
    }

    /// The layer lost `by_us` at its start and is now `len_us` long: keys keep their timeline place; those at or
    /// before the new start collapse into one key holding the value there.
    pub fn trim_start(&mut self, by_us: Us, len_us: Us) {
        if let Some(k) = self.keys.as_mut() {
            for ((keys, rest), limits) in k.each_mut().into_iter().zip(LIMITS) {
                cut_start(keys, by_us, rest, limits);
            }
        }
        self.fit(len_us);
    }

    /// The layer is now `len_us` long (an end trim, a new length): keys past the end go and one at the end holds the
    /// value there, and presets last at most the layer's length. Keys left with none go too.
    pub fn fit(&mut self, len_us: Us) {
        if let Some(k) = self.keys.as_mut() {
            for ((keys, rest), limits) in k.each_mut().into_iter().zip(LIMITS) {
                cut_end(keys, len_us, rest, limits);
            }
        }
        if self.keys.as_ref().is_some_and(MotionKeys::is_empty) {
            self.keys = None;
        }
        for u in [&mut self.enter, &mut self.exit, &mut self.looping]
            .into_iter()
            .flatten()
        {
            u.duration_us = u.duration_us.min(len_us).max(1);
        }
    }

    /// The layer was `old_len_us` long and lost the sorted, disjoint `cuts` (in its own time, within it); it is now
    /// `len_us` long. Keys keep their place on what stays, and values hold at each join where keys were cut away.
    /// O(keys × log cuts).
    pub fn cut(&mut self, cuts: &[(Us, Us)], old_len_us: Us, len_us: Us) {
        if let Some(k) = self.keys.as_mut() {
            for ((keys, rest), limits) in k.each_mut().into_iter().zip(LIMITS) {
                cut_ranges(keys, cuts, old_len_us, rest, limits);
            }
        }
        self.fit(len_us);
    }

    /// Each key moves to `at(t)` (a speed change rescales them, a separated camera moves them into the layer's time);
    /// then the motion fits a layer `len_us` long. `at` keeps keys in order; keys it lands on one moment keep the
    /// first.
    pub fn retime(&mut self, at: impl Fn(Us) -> Us, len_us: Us) {
        if let Some(k) = self.keys.as_mut() {
            for (keys, _) in k.each_mut() {
                keys.iter_mut().for_each(|key| key.t_us = at(key.t_us));
                keys.dedup_by(|later, earlier| later.t_us <= earlier.t_us);
            }
        }
        self.fit(len_us);
    }

    /// The layer's footage now plays at `new_speed` instead of `old_speed`, and the layer is `len_us` long: keys
    /// rescale so they stay on the same footage.
    pub fn rescale(&mut self, old_speed: f64, new_speed: f64, len_us: Us) {
        self.retime(|t| scaled(unscaled(t, old_speed), new_speed), len_us);
    }

    /// The part of a layer's motion from `from_us` into it, `len_us` long, for a piece of the layer that a cut kept:
    /// keys keep their place on the content, the enter stays only on a `first` piece and the exit on a `last` one.
    pub fn piece(&self, from_us: Us, len_us: Us, first: bool, last: bool) -> Motion {
        let mut m = self.clone();
        if !first {
            m.enter = None;
        }
        if !last {
            m.exit = None;
        }
        m.trim_start(from_us, len_us);
        m
    }
}

/// `m` unless it does nothing: what a layout keeps after an edit.
pub fn kept(m: Motion) -> Option<Motion> {
    (!m.is_empty()).then_some(m)
}

/// Why validation refuses `m`, as a model code, or None. `len_us` is the layer's length, or None for the camera
/// bubble (keys in source time; no presets). `text`: typewriter is for text layers only.
pub(crate) fn problem(m: &Motion, len_us: Option<Us>, text: bool) -> Option<&'static str> {
    if let Some(k) = &m.keys {
        for ((keys, _), (lo, hi)) in k.each().into_iter().zip(LIMITS) {
            if keys.len() > MAX_KEYS {
                return Some(crate::messages::code!("model.motion.tooManyKeys"));
            }
            if keys
                .iter()
                .any(|key| key.ease.is_some_and(|e| !e.is_valid()))
            {
                return Some(crate::messages::code!("model.motion.badEase"));
            }
            if keys
                .iter()
                .any(|key| !(key.v.is_finite() && key.v >= lo && key.v <= hi))
            {
                return Some(crate::messages::code!("model.motion.badValue"));
            }
            if keys.windows(2).any(|w| w[0].t_us >= w[1].t_us) {
                return Some(crate::messages::code!("model.motion.unsorted"));
            }
            // The camera bubble's keys (no layer length) stay within a day, as every other time does.
            if keys
                .iter()
                .any(|key| key.t_us < 0 || key.t_us > len_us.unwrap_or(MAX_TIME_US))
            {
                return Some(crate::messages::code!("model.motion.outside"));
            }
        }
    }
    let uses = [&m.enter, &m.exit, &m.looping];
    if len_us.is_none() && uses.iter().any(|u| u.is_some()) {
        return Some(crate::messages::code!("model.motion.cameraKeysOnly"));
    }
    for u in uses.into_iter().flatten() {
        if u.ease.is_some_and(|e| !e.is_valid()) {
            return Some(crate::messages::code!("model.motion.badEase"));
        }
        if !u.preset.is_well_formed() {
            return Some(crate::messages::code!("model.motion.badPreset"));
        }
        if u.duration_us < 1 || len_us.is_some_and(|len| u.duration_us > len) {
            return Some(crate::messages::code!("model.motion.presetTooLong"));
        }
        if !text && u.preset == crate::contrib::builtin::motion("typewriter") {
            return Some(crate::messages::code!("model.motion.typewriterText"));
        }
    }
    None
}
