//! The fixed modifiers and value animators (no expression language beyond these): wiggle, oscillate and marquee
//! add to a property; the counter, the typewriter and scrambles write text. Each is a pure function of the scene
//! time and its seed, so every render of a frame is the same, and none allocates: texts go into fixed buffers.
//! Randomness is `kl_hash` (the WGSL prelude's integer hash) over (seed, index, salt).

use arrayvec::ArrayString;

use super::format::{push, NumberFormat};
use super::Modifier;

/// The WGSL prelude's `kl_hash`, bit for bit.
pub fn hash(x: u32) -> u32 {
    let mut h = x;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    h
}

/// 0 to 1 (below 1), the same for the same seed, index and salt.
pub fn rand01(seed: u32, index: u32, salt: u32) -> f64 {
    f64::from(hash(index ^ hash(salt ^ hash(seed)))) / 4_294_967_296.0
}

/// Smooth noise `freq` times a second, within ±`amp`: seeded values at whole steps, eased between (quintic).
pub fn wiggle(t: f64, freq: f64, amp: f64, seed: u32) -> f64 {
    let s = t * freq;
    if !s.is_finite() {
        return 0.0;
    }
    let step = s.floor();
    let f = s - step;
    // Wrapping: a negative or huge step still picks a value.
    let k = step.clamp(i64::MIN as f64, i64::MAX as f64) as i64 as u32;
    let a = rand01(seed, k, 0x9e37) * 2.0 - 1.0;
    let b = rand01(seed, k.wrapping_add(1), 0x9e37) * 2.0 - 1.0;
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    amp * (a + (b - a) * u)
}

/// A sine `freq` times a second, `amp` high, starting `phase` cycles in (0.25 is a quarter turn), fading by `decay` a
/// second.
pub fn oscillate(t: f64, freq: f64, amp: f64, phase: f64, decay: f64) -> f64 {
    let v = amp
        * (std::f64::consts::TAU * (freq * t + phase)).sin()
        * (-decay.max(0.0) * t.max(0.0)).exp();
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// `speed` a second, from the scene's start.
pub fn marquee(t: f64, speed: f64) -> f64 {
    let v = speed * t;
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// A modifier as the evaluator runs it, with its seed decided.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModifierFn {
    Wiggle {
        freq: f64,
        amp: f64,
        seed: u32,
    },
    Oscillate {
        freq: f64,
        amp: f64,
        phase: f64,
        decay: f64,
    },
    Marquee {
        speed: f64,
    },
}

impl ModifierFn {
    /// A scene's modifier; a wiggle without a seed takes `seed`.
    pub fn of(m: &Modifier, seed: u32) -> Self {
        match *m {
            Modifier::Wiggle { freq, amp, seed: s } => Self::Wiggle {
                freq,
                amp,
                seed: s.unwrap_or(seed),
            },
            Modifier::Oscillate {
                freq,
                amp,
                phase,
                decay,
            } => Self::Oscillate {
                freq,
                amp,
                phase: phase.unwrap_or(0.0),
                decay: decay.unwrap_or(0.0),
            },
            Modifier::Marquee { speed } => Self::Marquee { speed },
        }
    }

    /// The same wiggle seeded for repeat instance `k` (each instance wiggles its own way); others as they are.
    pub fn reseeded(self, k: u32) -> Self {
        match self {
            Self::Wiggle { freq, amp, seed } => Self::Wiggle {
                freq,
                amp,
                seed: seed ^ hash(k.wrapping_add(0x51ed)),
            },
            other => other,
        }
    }

    /// What it adds at scene time `t`.
    pub fn at(self, t: f64) -> f64 {
        match self {
            Self::Wiggle { freq, amp, seed } => wiggle(t, freq, amp, seed),
            Self::Oscillate {
                freq,
                amp,
                phase,
                decay,
            } => oscillate(t, freq, amp, phase, decay),
            Self::Marquee { speed } => marquee(t, speed),
        }
    }
}

/// The counter's text: `prefix`, `v` as `pattern` writes it ([`NumberFormat::parse`]) and `suffix`, as much as fits.
pub fn count_text(v: f64, pattern: &str, prefix: &str, suffix: &str) -> ArrayString<64> {
    let mut out = ArrayString::new();
    count_into(v, NumberFormat::parse(pattern), prefix, suffix, &mut out);
    out
}

/// [`count_text`] into any buffer, appending.
pub fn count_into<const N: usize>(
    v: f64,
    format: NumberFormat,
    prefix: &str,
    suffix: &str,
    out: &mut ArrayString<N>,
) {
    for c in prefix.chars() {
        push(out, c);
    }
    format.write(v, out);
    for c in suffix.chars() {
        push(out, c);
    }
}

/// How many of `units` characters or words the typewriter shows at `progress` (0–1): a unit appears once its share
/// of the time has fully passed, and all of them at 1.
pub fn reveal_count(progress: f64, units: usize) -> usize {
    if progress.is_nan() {
        return 0;
    }
    ((progress.clamp(0.0, 1.0) * units as f64).floor() as usize).min(units)
}

/// A scramble of `text` into `out` (cleared first): at `progress` (0–1) the first share of its characters are
/// settled, and each of the rest is drawn from `chars` by (seed, position, step), so it changes with `step`. Spaces
/// stay spaces. As much as fits.
pub fn scramble_into<const N: usize>(
    text: &str,
    progress: f64,
    step: u32,
    chars: &[char],
    seed: u32,
    out: &mut ArrayString<N>,
) {
    out.clear();
    let total = text.chars().count();
    let settled = reveal_count(progress, total);
    for (i, c) in text.chars().enumerate() {
        let shown = if i < settled || c.is_whitespace() || chars.is_empty() {
            c
        } else {
            let pick = hash(seed ^ hash(i as u32 ^ hash(step))) as usize % chars.len();
            chars[pick]
        };
        if out.try_push(shown).is_err() {
            break;
        }
    }
}
