//! Shaders that aren't built in: agent drafts, the library, project-embedded copies.
//! Their text is `assemble`'s with the output check (`SANITIZE`) after `Params` and an entry that returns through it;
//! `guard::check` reads that text, and `guard::harden` adds the loop counter and has naga write it back out. The
//! hardened text is the only form of such a shader any device compiles.

use std::sync::Arc;

use klipolo_model::contrib::{ContribDef, MAX_PADDING_PX, PaddingSpec};
use klipolo_model::params::ParamKind;

use super::{
    Downsample, ParamLayout, ShaderEffect, ShaderKind, ShaderTrust, fnv1a64, validate_wgsl,
};
use crate::registry::guard::{self, Diagnostic};

/// The output check every checked shader's entry returns through: what a shader draws is finite, 0–1 and
/// premultiplied, so valid output is unchanged and nothing else reaches the frame.
pub const SANITIZE: &str = r#"// Non-finite values (exponent all ones) read as 0 whatever the compiler's float mode; then 0–1, rgb ≤ a.
fn kl_finite(x: f32) -> f32 {
    return select(x, 0.0, (bitcast<u32>(x) & 0x7f800000u) == 0x7f800000u);
}
fn kl_sanitize(c: vec4<f32>) -> vec4<f32> {
    let f = clamp(vec4<f32>(kl_finite(c.r), kl_finite(c.g), kl_finite(c.b), kl_finite(c.a)), vec4<f32>(0.0), vec4<f32>(1.0));
    return vec4<f32>(min(f.rgb, vec3<f32>(f.a)), f.a);
}
"#;

/// The text `guard::check` reads, after the definition's own checks (its settings and their conditions).
pub(crate) fn source(def: &ContribDef, kind: ShaderKind, body: &str) -> Result<String, String> {
    def.check_conditions()?;
    let layout = ParamLayout::of(&def.params)?;
    Ok(layout.source(def, kind, body, ShaderTrust::Checked))
}

impl ShaderEffect {
    /// A shader that isn't built in, checked and hardened. A built-in id or a protective definition is refused
    /// (only built-ins may hide what they cover), a padding that isn't a filter's or is out of range, a `resolution` or
    /// `fps` that isn't a generator's or an `fps` outside 1–60, then everything `guard::check` and
    /// `guard::harden` refuse.
    pub fn assemble_untrusted(
        def: &ContribDef,
        kind: ShaderKind,
        body: &str,
    ) -> Result<Self, Vec<Diagnostic>> {
        Self::assemble_untrusted_with_notes(def, kind, body).map(|(shader, _)| shader)
    }

    /// [`assemble_untrusted`](Self::assemble_untrusted), with the guard's notes on a shader that passed
    /// (`Checked::notes`: the colours and points it measures at probes), which the render worker forwards.
    pub fn assemble_untrusted_with_notes(
        def: &ContribDef,
        kind: ShaderKind,
        body: &str,
    ) -> Result<(Self, Vec<Diagnostic>), Vec<Diagnostic>> {
        if def.id.is_builtin() {
            // Quoted and cut at 64 characters, as `place` names a malformed id.
            let id: String = def.id.as_str().chars().take(64).collect();
            return Err(vec![
                Diagnostic::error("rule.builtin", format!("{id:?} is a built-in id."))
                    .hint("Give the effect an id of its own plugin."),
            ]);
        }
        if def.protective {
            return Err(vec![
                Diagnostic::error(
                    "rule.protective",
                    "Only built-in effects can be protective (hide what they cover).",
                )
                .hint("Leave `protective` out."),
            ]);
        }
        let padding = padding(def, kind)?;
        let (resolution, fps) = super::timing::timing(def, kind)
            .map_err(|r| vec![Diagnostic::error(r.code, r.message).hint(r.hint)])?;
        let checked = guard::check(def, kind, body)?;
        let layout =
            ParamLayout::of(&def.params).map_err(|e| vec![Diagnostic::error("rule.params", e)])?;
        let downsample = Downsample::resolve(def, kind, &layout)
            .map_err(|e| vec![Diagnostic::error("rule.downsample", e)])?;
        let hardened = guard::harden::harden(&checked)?;
        let shader = Self {
            key: fnv1a64(hardened.as_bytes()),
            source: hardened.into(),
            id: def.id.clone(),
            name: def.name.clone(),
            kind,
            params: def.params.clone().into(),
            layout,
            then: None,
            downsample,
            protective: false,
            padding,
            resolution,
            fps,
            trust: ShaderTrust::Checked,
            animates: checked.facts.animates,
            facts: Some(Arc::new(checked.facts)),
            envelope: None,
            runs_at: Default::default(),
            cost: None,
        };
        Ok((shader, checked.notes))
    }
}

impl crate::registry::Registry {
    /// The built-ins plus `shaders`, each under its definition: contributions that aren't built in, checked and
    /// hardened on this computer (`assemble_untrusted`, never text read from anywhere). The render worker's
    /// registry; one that names a built-in id or isn't checked is left out. The app's registry from loaded
    /// folders is `with_contributions` (`pack`).
    pub fn with_checked(
        shaders: impl IntoIterator<Item = (ContribDef, ShaderEffect)>,
    ) -> Arc<Self> {
        // The built-ins are assembled once per process; this copies their tables.
        let mut registry = (*Self::builtin()).clone();
        for (def, shader) in shaders {
            if def.id.is_builtin() || shader.trust != ShaderTrust::Checked || shader.id != def.id {
                continue;
            }
            registry.shaders.insert(def.id.clone(), Arc::new(shader));
            registry.catalog = std::mem::take(&mut registry.catalog).with(def);
        }
        Arc::new(registry)
    }
}

/// The definition's margin past its layer: a filter's only, `px`, `perUnit` and `maxPx` finite and 0 or more,
/// `maxPx` at most [`MAX_PADDING_PX`], and the setting it grows with a number setting.
fn padding(def: &ContribDef, kind: ShaderKind) -> Result<Option<PaddingSpec>, Vec<Diagnostic>> {
    let Some(pad) = &def.padding else {
        return Ok(None);
    };
    let refuse = |message: String, hint: &str| {
        Err(vec![Diagnostic::error("rule.padding", message).hint(hint)])
    };
    if kind != ShaderKind::Filter {
        return refuse(
            "Only a filter can draw outside its layer (`padding`).".into(),
            "Leave `padding` out.",
        );
    }
    if klipolo_model::validate::check_padding(def).is_err() || pad.max_px > MAX_PADDING_PX {
        return refuse(
            format!("`padding` takes 0 to {MAX_PADDING_PX} pixels."),
            "Give `px`, `perUnit` and `maxPx` as numbers of 0 or more, `maxPx` at most 256.",
        );
    }
    if let Some(key) = &pad.param
        && !matches!(
            def.param(key).map(|s| &s.kind),
            Some(ParamKind::Float { .. })
        )
    {
        return refuse(
            format!("`padding` grows with \"{key}\", which isn't a number setting."),
            "Name a number setting, or leave `param` out.",
        );
    }
    Ok(Some(pad.clone()))
}

impl ShaderEffect {
    /// Whether a device may compile this shader's text: a built-in's must validate; anything else's must be the
    /// hardened module (`guard::harden::verify`: the host's entry points, every loop's counter check, the output
    /// check). The effect stage's compile asks this first, so text that skipped hardening never reaches a device,
    /// whatever built the `ShaderEffect`.
    pub fn compilable(&self) -> Result<(), String> {
        match (self.trust, self.id.is_builtin()) {
            (ShaderTrust::Builtin, true) => validate_wgsl(&self.source),
            (ShaderTrust::Builtin, false) => Err(format!(
                "{} isn't built in and wasn't checked, so it isn't drawn.",
                self.id
            )),
            (ShaderTrust::Checked, _) => guard::harden::verify(&self.source)
                .map(drop)
                .map_err(|why| format!("{} isn't a hardened shader: {why}", self.id)),
        }
    }
}

#[cfg(feature = "test-support")]
impl crate::registry::Registry {
    /// The built-ins plus `shader` under `def` (tests: a registry that draws one shader that isn't built in). Only in
    /// builds with the `test-support` feature, which the engine's own tests turn on.
    #[doc(hidden)]
    pub fn builtin_with_test_shader(def: &ContribDef, shader: ShaderEffect) -> Arc<Self> {
        Self::builtin_with_test_shaders([(def.clone(), shader)])
    }

    /// The built-ins plus each shader under its definition (tests; `test-support` only, as above).
    #[doc(hidden)]
    pub fn builtin_with_test_shaders(
        shaders: impl IntoIterator<Item = (ContribDef, ShaderEffect)>,
    ) -> Arc<Self> {
        let mut registry = (*Self::builtin()).clone();
        for (def, shader) in shaders {
            registry.shaders.insert(def.id.clone(), Arc::new(shader));
            registry.catalog = std::mem::take(&mut registry.catalog).with(def);
        }
        Arc::new(registry)
    }
}

#[cfg(test)]
mod tests {
    use klipolo_model::contrib::{ContribDef, ContribKind};

    use crate::registry::{Registry, ShaderEffect, ShaderKind, builtin_builds};

    /// A registry with contributions copies the built-ins assembled once per process, never assembles them again.
    #[test]
    fn the_built_ins_are_assembled_once() {
        let def = ContribDef::new(
            "lib.k3f9q2ma/generator.once.v1",
            ContribKind::Generator,
            "Once",
        );
        let body = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }";
        for _ in 0..3 {
            let shader =
                ShaderEffect::assemble_untrusted(&def, ShaderKind::Generator, body).unwrap();
            let r = Registry::with_checked([(def.clone(), shader)]);
            assert!(r.shader(&def.id).is_some());
        }
        assert_eq!(builtin_builds(), 1);
    }
}
