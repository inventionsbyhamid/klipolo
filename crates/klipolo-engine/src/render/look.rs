//! Looks as Cap's parametric colour grade. `strength` is the grade's intensity and also scales the grain (which
//! Cap's intensity leaves alone), so strength 0 is no look at all. The grades live in `registry`.

use crate::registry::LookGrade;
use crate::vendor::cap::compat::ColorCorrection;

pub fn grade(g: &LookGrade, strength: f64) -> ColorCorrection {
    let strength = strength.clamp(0.0, 1.0) as f32;
    ColorCorrection {
        intensity: strength,
        temperature: g.temperature,
        tint: g.tint,
        exposure: g.exposure,
        contrast: g.contrast,
        saturation: g.saturation,
        fade: g.fade,
        split_tone: g.split_tone,
        vignette: g.vignette,
        grain: g.grain * strength,
        ..ColorCorrection::default()
    }
}
