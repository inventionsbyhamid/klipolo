//! Layer motion at one instant, in this order: the static layout per format, then
//! the enter, exit and loop presets, then the keys. Pure and allocation-free: the plan calls it for every moving layer
//! of every frame. Enter and exit presets move a box by exact distances in the format being drawn, so a slide from the
//! top starts just above the canvas in 16:9, 9:16 and 1:1 alike.

use klipolo_model::layout::BoxN;
use klipolo_model::motion::{Ease, Motion, MotionUse, sample};
use klipolo_model::params::ParamValue;
use klipolo_model::time::Us;

use crate::registry::{Preset, Registry};

/// The largest blur `blur-in` starts from, in 1080p pixels.
pub const BLUR_IN_PX: f64 = 24.0;

/// A layer's box and look after its motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moved {
    pub rect: BoxN,
    /// Degrees added to the layout's rotation.
    pub rotation: f64,
    /// Multiplies the layout's opacity.
    pub opacity: f64,
    /// About the box's centre (text layers draw their font scaled by it).
    pub scale: f64,
    /// How much of a text is shown (the typewriter preset); 1 elsewhere.
    pub reveal: f64,
    /// A blur radius in 1080p pixels while the `blur-in` preset runs; 0 elsewhere.
    pub blur_px: f64,
}

impl Moved {
    /// `rect` as it is: a layer that doesn't move.
    pub fn still(rect: BoxN) -> Self {
        Self {
            rect,
            rotation: 0.0,
            opacity: 1.0,
            scale: 1.0,
            reveal: 1.0,
            blur_px: 0.0,
        }
    }

    /// The moved box at the size it had before the scale (`size`), about the same centre: a text layer keeps its box
    /// (so its texture is drawn once, not per frame) and draws it scaled by `scale` instead.
    pub fn unscaled(&self, size: BoxN) -> BoxN {
        if self.rect.w == size.w && self.rect.h == size.h {
            return self.rect;
        }
        let (cx, cy) = (
            self.rect.x + self.rect.w / 2.0,
            self.rect.y + self.rect.h / 2.0,
        );
        BoxN {
            x: cx - size.w / 2.0,
            y: cy - size.h / 2.0,
            w: size.w,
            h: size.h,
        }
    }

    /// How many times bigger than `rect` the largest box `m` reaches is (at least 1): effect targets, image decodes
    /// and overlay video are sized for it once, so a scaling layer never reallocates them per frame.
    pub fn reach_from_here(&self, m: &Motion) -> f64 {
        if self.scale > 1e-9 {
            (reach(m) / self.scale).max(1.0)
        } else {
            1.0
        }
    }
}

fn text(u: &MotionUse, key: &str, default: &'static str) -> &'static str {
    match u.params.get(key) {
        Some(ParamValue::Text(t)) if t == "top" => "top",
        Some(ParamValue::Text(t)) if t == "left" => "left",
        Some(ParamValue::Text(t)) if t == "right" => "right",
        Some(ParamValue::Text(t)) if t == "bottom" => "bottom",
        _ => default,
    }
}

fn number(u: &MotionUse, key: &str, default: f64) -> f64 {
    match u.params.get(key) {
        Some(ParamValue::Number(n)) if n.is_finite() => *n,
        _ => default,
    }
}

/// `e` runs 0 (hidden) → 1 (at rest) through one enter or exit. Both ends run on every frame (an end outside its span
/// at `e` = 1), so each combines with what the other did rather than replacing it: opacity and scale multiply,
/// offsets add, the typewriter keeps the shorter text and `blur-in` the larger blur.
fn appear(m: &mut Moved, preset: Preset, u: &MotionUse, e: f64) {
    let r = m.rect;
    match preset {
        Preset::Fade => m.opacity *= e,
        Preset::Slide => match text(u, "from", "bottom") {
            "top" => m.rect.y -= (r.y + r.h) * (1.0 - e),
            "left" => m.rect.x -= (r.x + r.w) * (1.0 - e),
            "right" => m.rect.x += (1.0 - r.x) * (1.0 - e),
            _ => m.rect.y += (1.0 - r.y) * (1.0 - e),
        },
        Preset::Pop => {
            m.opacity *= e.min(1.0);
            m.scale *= 0.85 + 0.15 * e;
        }
        Preset::Zoom => {
            m.opacity *= e.clamp(0.0, 1.0);
            m.scale *= 0.5 + 0.5 * e;
        }
        Preset::Drop => m.rect.y -= (r.y + r.h) * (1.0 - e),
        Preset::Bounce => m.scale *= 0.3 + 0.7 * e,
        Preset::BlurIn => {
            m.opacity *= e.clamp(0.0, 1.0);
            m.blur_px = m.blur_px.max(BLUR_IN_PX * (1.0 - e.clamp(0.0, 1.0)));
        }
        Preset::Typewriter => m.reveal = m.reveal.min(e.clamp(0.0, 1.0)),
        _ => {}
    }
}

/// One cycle of a loop, `phase` 0–1 through it, at `strength` (1 is the preset's own size).
fn wobble(m: &mut Moved, preset: Preset, phase: f64, strength: f64) {
    let wave = (phase * std::f64::consts::TAU).sin();
    match preset {
        Preset::Float => m.rect.y += 0.01 * strength * wave,
        Preset::Pulse => m.scale *= 1.0 + 0.04 * strength * wave,
        Preset::Shake => m.rect.x += 0.006 * strength * (phase * 3.0 * std::f64::consts::TAU).sin(),
        Preset::Wiggle => m.rotation += 3.0 * strength * wave,
        Preset::Spin => m.rotation += 360.0 * phase * strength.max(0.0).ceil(),
        _ => {}
    }
}

/// The default ease of each preset (the person's ease wins).
fn ease(preset: Preset, u: &MotionUse) -> Ease {
    u.ease.unwrap_or(match preset {
        Preset::Pop => Ease::BackOut,
        Preset::Drop | Preset::Bounce => Ease::BounceOut,
        Preset::Fade | Preset::Typewriter => Ease::Linear,
        _ => Ease::Power2Out,
    })
}

/// `m` at `local_us` into a layer `len_us` long whose box in this format is `rect`. Presets this build doesn't know
/// do nothing; the caller names them in `missing`.
pub fn apply(m: &Motion, registry: &Registry, local_us: Us, len_us: Us, rect: BoxN) -> Moved {
    let mut out = Moved::still(rect);
    if let Some(u) = &m.enter
        && let Some(p) = registry.motion_preset(&u.preset)
    {
        let t = (local_us as f64 / u.duration_us.max(1) as f64).clamp(0.0, 1.0);
        appear(&mut out, p, u, ease(p, u).apply(t));
    }
    if let Some(u) = &m.exit
        && let Some(p) = registry.motion_preset(&u.preset)
    {
        let left = (len_us - local_us) as f64 / u.duration_us.max(1) as f64;
        appear(&mut out, p, u, ease(p, u).apply(left.clamp(0.0, 1.0)));
    }
    if let Some(u) = &m.looping
        && let Some(p) = registry.motion_preset(&u.preset)
    {
        let cycle = u.duration_us.max(1);
        let phase = local_us.rem_euclid(cycle) as f64 / cycle as f64;
        wobble(&mut out, p, phase, number(u, "strength", 1.0));
    }
    let k = sample(m, local_us, len_us);
    out.rect.x += k.dx;
    out.rect.y += k.dy;
    out.scale *= k.scale;
    out.rotation += k.rotation;
    out.opacity *= k.opacity;
    if (out.scale - 1.0).abs() > 1e-9 {
        let (cx, cy) = (out.rect.x + out.rect.w / 2.0, out.rect.y + out.rect.h / 2.0);
        out.rect.w *= out.scale;
        out.rect.h *= out.scale;
        out.rect.x = cx - out.rect.w / 2.0;
        out.rect.y = cy - out.rect.h / 2.0;
    }
    out
}

/// The most `reach` gives, whatever the motion (a second line behind validation): the keys' largest
/// scale and half again. Past it a layer's effects run smaller than it is drawn (they never size targets past it).
pub const MAX_REACH: f64 = klipolo_model::motion::LIMITS[2].1 * 1.5;

/// The largest scale `m` reaches: an effect target is allocated once at this size. The keys' scale, a loop's pulse
/// and an enter's or exit's eased overshoot (pop's back.out, an elastic zoom) multiply, so their bounds multiply too.
/// At most [`MAX_REACH`]; 1 for anything not finite.
pub fn reach(m: &Motion) -> f64 {
    let pulse = m
        .looping
        .as_ref()
        .map_or(1.0, |u| 1.0 + 0.04 * number(u, "strength", 1.0).abs());
    let ends = [&m.enter, &m.exit]
        .into_iter()
        .flatten()
        .map(overshoot)
        .product::<f64>();
    let reach = klipolo_model::motion::max_key_scale(m) * pulse * ends;
    if reach.is_nan() {
        1.0
    } else {
        reach.clamp(1.0, MAX_REACH)
    }
}

/// Whether `m` can turn its layer while it plays: a `spin` or `wiggle` loop, or rotation keys that don't all hold one
/// angle (a turned glass card's grab is the turned box's own when its angle is fixed, the square
/// that holds every turn only when it can change). Found by the loop's word, without the registry, as `overshoot` is.
pub fn turns(m: &Motion) -> bool {
    let looping = m.looping.as_ref().is_some_and(|u| {
        matches!(
            u.preset.as_str().strip_prefix("klipolo.builtin/motion."),
            Some("spin" | "wiggle")
        )
    });
    let keys = m.keys.as_ref().is_some_and(|k| {
        k.rotation
            .first()
            .is_some_and(|a| k.rotation.iter().any(|b| b.v != a.v))
    });
    looping || keys
}

/// The largest scale a built-in enter or exit gives (1 for those that don't grow past their size): its scale at
/// rest is 1, and an ease that overshoots (`peak` > 1) carries it past. Found by the id's word, without the registry
/// (a plugin's preset draws nothing, so it never grows).
fn overshoot(u: &MotionUse) -> f64 {
    let (preset, from) = match u.preset.as_str().strip_prefix("klipolo.builtin/motion.") {
        Some("pop") => (Preset::Pop, 0.85),
        Some("zoom") => (Preset::Zoom, 0.5),
        Some("bounce") => (Preset::Bounce, 0.3),
        _ => return 1.0,
    };
    (from + (1.0 - from) * peak(ease(preset, u))).max(1.0)
}

/// An upper bound of `e` over 0–1: back eases overshoot by about 10 %, elastic ones by under 40 %, a cubic Bézier
/// stays within its control points, which validation keeps within `EASE_Y` (clamped here too), and a spring's first
/// swing is its highest (under 1.86 at the least damping validation allows).
fn peak(e: Ease) -> f64 {
    match e {
        Ease::BackIn | Ease::BackOut | Ease::BackInOut => 1.11,
        Ease::ElasticIn | Ease::ElasticOut | Ease::ElasticInOut => 1.4,
        Ease::CubicBezier { y1, y2, .. } => {
            y1.max(y2).max(1.0).min(klipolo_model::motion::EASE_Y.1)
        }
        Ease::Spring {
            stiffness,
            damping,
            mass,
        } => klipolo_model::motion::spring_peak(stiffness, damping, mass),
        _ => 1.0,
    }
}
