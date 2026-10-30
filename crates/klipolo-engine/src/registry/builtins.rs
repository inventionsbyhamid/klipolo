//! The registry's built-in tables (moved from `registry/mod.rs`, no behaviour change): look grades, motion presets,
//! audio filter chains and the built-in shaders, and the one registry they build.

use std::collections::HashMap;
use std::sync::Arc;

use klipolo_model::contrib::{Catalog, ContribId, builtin};

use super::{LookGrade, Preset, Registry, ShaderEffect, ShaderKind};

const NO_GRADE: LookGrade = LookGrade {
    temperature: 0.0,
    tint: 0.0,
    exposure: 0.0,
    contrast: 0.0,
    saturation: 0.0,
    fade: 0.0,
    split_tone: 0.0,
    vignette: 0.0,
    grain: 0.0,
};

/// The look table, as data.
const LOOKS: [(&str, LookGrade); 7] = [
    (
        "warm",
        LookGrade {
            temperature: 0.35,
            tint: 0.05,
            ..NO_GRADE
        },
    ),
    (
        "cool",
        LookGrade {
            temperature: -0.35,
            ..NO_GRADE
        },
    ),
    (
        "mono",
        LookGrade {
            saturation: -1.0,
            contrast: 0.1,
            ..NO_GRADE
        },
    ),
    (
        "vintage",
        LookGrade {
            fade: 0.25,
            split_tone: 0.3,
            saturation: -0.2,
            grain: 0.15,
            ..NO_GRADE
        },
    ),
    (
        "vivid",
        LookGrade {
            saturation: 0.35,
            contrast: 0.15,
            ..NO_GRADE
        },
    ),
    (
        "cinema",
        LookGrade {
            contrast: 0.2,
            split_tone: 0.5,
            vignette: 0.3,
            saturation: -0.1,
            ..NO_GRADE
        },
    ),
    (
        "romcom",
        LookGrade {
            temperature: 0.2,
            fade: 0.15,
            saturation: 0.1,
            exposure: 0.05,
            ..NO_GRADE
        },
    ),
];

/// `builtin::MOTION_PRESETS` then `builtin::MOTION_LOOPS`, in their order (a test holds them equal).
const MOTION: [(&str, Preset); 13] = [
    ("fade", Preset::Fade),
    ("slide", Preset::Slide),
    ("pop", Preset::Pop),
    ("zoom", Preset::Zoom),
    ("drop", Preset::Drop),
    ("bounce", Preset::Bounce),
    ("blur-in", Preset::BlurIn),
    ("typewriter", Preset::Typewriter),
    ("float", Preset::Float),
    ("pulse", Preset::Pulse),
    ("shake", Preset::Shake),
    ("wiggle", Preset::Wiggle),
    ("spin", Preset::Spin),
];

/// The filter chains, run live in the mixer with the speed change (`audio::stretch`).
const AUDIO: [(&str, &str); 3] = [
    (
        "voice",
        "highpass=f=80,equalizer=f=3000:t=q:w=1:g=3,acompressor=threshold=-18dB:ratio=3:attack=5:release=50",
    ),
    ("warm", "lowshelf=f=200:g=3,lowpass=f=9000"),
    ("telephone", "highpass=f=300,lowpass=f=3400"),
];

/// The separable blur's shared function; `build_builtin` draws it across, then down.
const BLUR: &str = include_str!("wgsl/blur.wgsl");

const SHADERS: [(&str, ShaderKind, &str); 8] = [
    (
        builtin::EFFECT_MASK,
        ShaderKind::Filter,
        include_str!("wgsl/mask.wgsl"),
    ),
    (
        builtin::EFFECT_PIXELATE,
        ShaderKind::Filter,
        include_str!("wgsl/pixelate.wgsl"),
    ),
    (
        builtin::EFFECT_VIGNETTE,
        ShaderKind::Filter,
        include_str!("wgsl/vignette.wgsl"),
    ),
    (
        builtin::GENERATOR_GRADIENT,
        ShaderKind::Generator,
        include_str!("wgsl/gradient.wgsl"),
    ),
    (
        builtin::TRANSITION_DISSOLVE,
        ShaderKind::Transition,
        include_str!("wgsl/dissolve.wgsl"),
    ),
    (
        builtin::TRANSITION_DIP,
        ShaderKind::Transition,
        include_str!("wgsl/dip.wgsl"),
    ),
    (
        builtin::TRANSITION_PUSH,
        ShaderKind::Transition,
        include_str!("wgsl/push.wgsl"),
    ),
    (
        builtin::TRANSITION_WIPE,
        ShaderKind::Transition,
        include_str!("wgsl/wipe.wgsl"),
    ),
];

/// Times the built-ins were assembled (once per process, `Registry::builtin`).
static BUILDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many times this process assembled the built-ins (tests).
#[doc(hidden)]
pub fn builtin_builds() -> usize {
    BUILDS.load(std::sync::atomic::Ordering::Relaxed)
}

pub(super) fn build_builtin() -> Registry {
    BUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let catalog = Catalog::builtin().clone();
    let (blur_id, blur) = blur(&catalog);
    // The shader presets: built-ins read from their data folders (`presets`).
    let shipped = super::presets::SHADER_BODIES.iter().map(|&(local, body)| {
        (
            ContribId::builtin(local),
            super::presets::kind_of(local),
            body,
        )
    });
    let shaders = SHADERS
        .iter()
        .map(|&(id, kind, body)| (ContribId::from(id), kind, body))
        .chain(shipped)
        .map(|(id, kind, body)| {
            let def = catalog
                .get(&id)
                .expect("every built-in shader is in the catalog");
            let effect = ShaderEffect::builtin(def, kind, body).expect("built-in shaders assemble");
            (id, Arc::new(effect))
        })
        .chain(std::iter::once((blur_id, blur.clone())))
        .collect();
    Registry {
        shaders,
        scenes: HashMap::new(),
        blur,
        looks: LOOKS
            .iter()
            .map(|&(word, grade)| (builtin::look(word), grade))
            .collect(),
        motion: MOTION
            .iter()
            .map(|&(word, preset)| (builtin::motion(word), preset))
            .collect(),
        audio: AUDIO
            .iter()
            .map(|&(word, filter)| (builtin::audio_effect(word), filter))
            .collect(),
        packs: HashMap::new(),
        catalog,
    }
}

/// Blur as two passes, across then down, from `BLUR`.
fn blur(catalog: &Catalog) -> (ContribId, Arc<ShaderEffect>) {
    let id = ContribId::from(builtin::EFFECT_BLUR);
    let def = catalog.get(&id).expect("blur is in the catalog");
    let along = |axis: &str| {
        let body = format!(
            "{BLUR}\nfn effect(uv: vec2<f32>) -> vec4<f32> {{\n    return blur_along(uv, {axis});\n}}\n"
        );
        ShaderEffect::builtin(def, ShaderKind::Filter, &body).expect("blur assembles")
    };
    let effect = along("vec2<f32>(1.0, 0.0)").followed_by(along("vec2<f32>(0.0, 1.0)"));
    (id, Arc::new(effect))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_motion_preset_is_drawn() {
        let words: Vec<&str> = builtin::MOTION_PRESETS
            .iter()
            .chain(&builtin::MOTION_LOOPS)
            .copied()
            .collect();
        assert_eq!(words, MOTION.map(|(word, _)| word));
        let registry = Registry::builtin();
        assert_eq!(
            registry.motion_preset(&builtin::motion("blur-in")),
            Some(Preset::BlurIn)
        );
        assert_eq!(registry.motion_preset(&builtin::motion("swirl")), None);
    }

    #[test]
    fn audio_effects_resolve_to_t6b_filter_chains() {
        let registry = Registry::builtin();
        assert_eq!(
            registry.audio_filter(&builtin::audio_effect("telephone")),
            Some("highpass=f=300,lowpass=f=3400")
        );
        assert_eq!(
            registry.audio_filter(&"com.acme.fx/audio.robot".into()),
            None
        );
    }
}
