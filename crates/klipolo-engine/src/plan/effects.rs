//! Per-clip and per-layer effects: fades to black, looks, and effect stacks as shader passes, with the
//! margin a padded filter draws past its layer. A checked contribution's settings are held inside what this
//! computer measured (`clamped`); one the catalogue knows but that can't draw yet is noted as being
//! checked, not as missing.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use klipolo_model::contrib::{ContribId, MAX_PADDING_PX};
use klipolo_model::layout::BoxN;
use klipolo_model::params::{ParamKind, ParamValue};
use klipolo_model::project::Project;
use klipolo_model::time::{Us, us_to_secs};
use klipolo_model::timeline::{Clip, EffectInstance};

use crate::registry::{Registry, ShaderEffect, ShaderKind, fnv1a64};

use super::drawn::{Drawn, drawn};
use super::strings::Table;
use super::{EffectPass, LookPlan, LutRef, MissingContrib, ParamBlock};
use klipolo_model::contrib::ContribKind;

mod clamp;
pub use clamp::clamped;

/// Black over the base layers for clip fades: 1 at the very start of a fade-in, 0 once faded in.
pub(super) fn fade_alpha(clip: &Clip, local_us: Us) -> f64 {
    let mut visible = 1.0f64;
    if clip.fade_in_us > 0 {
        visible = visible.min(local_us as f64 / clip.fade_in_us as f64);
    }
    if clip.fade_out_us > 0 {
        visible = visible.min((clip.duration_us() - local_us) as f64 / clip.fade_out_us as f64);
    }
    1.0 - visible.clamp(0.0, 1.0)
}

/// The clip's look and LUT, or None when it has neither. A look this build doesn't have is listed in `missing`.
pub(super) fn look_plan(
    p: &Project,
    registry: &Registry,
    clip: &Clip,
    s: &mut Table,
    missing: &mut Vec<MissingContrib>,
) -> Option<LookPlan> {
    let lut = clip
        .look
        .lut_id
        .as_deref()
        .and_then(|id| p.luts.iter().find(|l| l.id == id))
        .map(|l| LutRef {
            id: s.str(&l.id),
            path: s.path(&l.path, registry),
            strength: clip.look.lut_strength,
        });
    let look = match &clip.look.preset {
        Some(id) => match registry.look(id) {
            Some(grade) => Some((s.str(id.as_str()), grade)),
            None => {
                note_missing(missing, ContribKind::Look, s.str(id.as_str()));
                None
            }
        },
        None => None,
    };
    (look.is_some() || lut.is_some()).then_some(LookPlan {
        look,
        strength: clip.look.strength,
        lut,
    })
}

/// When a layer's passes run: `local_us` into a layer `duration_us` long, in a project of `fps` frames a second
/// (`kl.fps` on every pass, ABI v1.1).
#[derive(Clone, Copy, Debug)]
pub(super) struct Clock {
    pub local_us: Us,
    pub duration_us: Us,
    pub fps: u32,
}

impl Clock {
    /// `kl.time` and `kl.progress` at this instant, and `kl.duration` and `kl.fps`.
    fn times(self) -> (f32, f32, f32, f32) {
        (
            us_to_secs(self.local_us.max(0)) as f32,
            (self.local_us as f64 / self.duration_us.max(1) as f64).clamp(0.0, 1.0) as f32,
            us_to_secs(self.duration_us.max(0)) as f32,
            self.fps as f32,
        )
    }
}

/// A stack's enabled effects that this build can draw, as passes in order. The rest are left out, and the ones it
/// doesn't have are listed in `missing`. A stack without effects allocates nothing.
pub(super) fn effect_passes(
    registry: &Registry,
    layer_id: &str,
    stack: &[EffectInstance],
    clock: Clock,
    s: &mut Table,
    missing: &mut Vec<MissingContrib>,
) -> Vec<EffectPass> {
    let mut passes = Vec::new();
    for e in stack.iter().filter(|e| e.enabled) {
        match registry
            .shader(&e.effect)
            .filter(|sh| sh.kind == ShaderKind::Filter)
        {
            Some(shader) => {
                if passes.capacity() == 0 {
                    // One allocation for the whole stack.
                    passes.reserve_exact(stack.len());
                }
                let ids = (layer_id, e.id.as_str());
                passes.push(pass(registry, shader, &e.params, ids, clock, (s, missing)));
            }
            None => note_not_drawn(registry, missing, ContribKind::Effect, &e.effect, s),
        }
    }
    passes
}

/// One pass of `shader` at `clock` for (layer, effect) `ids`, with the margin its padding gives these settings (a
/// registry lookup only for a padded filter). A checked shader's settings are clamped into its envelope first, once,
/// so the margin and the packed settings both see what is drawn; an instance that had to be clamped is noted
/// as being checked (its re-measure is the app's).
pub(super) fn pass(
    registry: &Registry,
    shader: &Arc<ShaderEffect>,
    values: &BTreeMap<String, ParamValue>,
    (layer_id, effect_id): (&str, &str),
    clock: Clock,
    (s, missing): (&mut Table, &mut Vec<MissingContrib>),
) -> EffectPass {
    let values = clamped(values, &shader.params, shader.envelope.as_deref());
    if let Cow::Owned(_) = values {
        let kind = match shader.kind {
            ShaderKind::Generator => ContribKind::Generator,
            ShaderKind::Transition => ContribKind::Transition,
            ShaderKind::Filter => ContribKind::Effect,
        };
        note_checking(missing, kind, s.str(shader.id.as_str()));
    }
    let values = &*values;
    let (time_s, progress, duration_s, fps) = clock.times();
    EffectPass {
        shader: shader.clone(),
        params: pack_params(shader, values),
        time_s,
        progress,
        frame: None,
        duration_s,
        fps,
        step_fps: 0.0,
        seed: seed_of(layer_id, effect_id),
        pad_px: shader.padding.as_ref().map_or(0.0, |pad| {
            // Every registry keeps each shader's definition: a padded one without it would clip its glow.
            let def = registry.catalog().get(&shader.id);
            debug_assert!(def.is_some(), "{} has no definition", shader.id);
            def.map_or(0.0, |def| pad.px_for(values, def))
        }),
    }
}

/// How far a layer's effects draw past its box, in 1080p pixels: its passes' margins added up (a glow then a shadow
/// need both), at most `MAX_PADDING_PX`.
pub(super) fn pad_of(passes: &[EffectPass]) -> f64 {
    let sum: f64 = passes.iter().map(|p| f64::from(p.pad_px)).sum();
    sum.min(f64::from(MAX_PADDING_PX))
}

/// `dest` grown by `pad_px` 1080p pixels on each side, in fractions of a `canvas`-pixel frame drawn at `px` pixels per
/// 1080p pixel. Not clipped: the compositor keeps only the margin inside the frame.
pub fn padded_dest(dest: BoxN, pad_px: f64, canvas: (u32, u32), px: f64) -> BoxN {
    let (dx, dy) = (
        pad_px * px / f64::from(canvas.0.max(1)),
        pad_px * px / f64::from(canvas.1.max(1)),
    );
    BoxN {
        x: dest.x - dx,
        y: dest.y - dy,
        w: dest.w + 2.0 * dx,
        h: dest.h + 2.0 * dy,
    }
}

/// The built-in blur at `radius_px` (1080p pixels) as a pass at `clock`: the `blur-in` preset while it runs. Packed
/// straight into the block (no settings map): no allocation.
pub(super) fn blur_pass(
    registry: &Registry,
    layer_id: &str,
    radius_px: f64,
    clock: Clock,
) -> Option<EffectPass> {
    let shader = registry.blur();
    let mut params = ParamBlock::zeroed(shader.layout.size);
    let &(i, _, at) = shader.layout.fields.first()?;
    let ParamKind::Float { min, max, .. } = shader.params[i].kind else {
        return None;
    };
    params.put_f32s(at, &[radius_px.clamp(min, max) as f32]);
    let (time_s, progress, duration_s, fps) = clock.times();
    Some(EffectPass {
        time_s,
        progress,
        duration_s,
        fps,
        seed: seed_of(layer_id, "motion.blur-in"),
        ..EffectPass::host(shader.clone(), params)
    })
}

/// Lists contribution `id` of `kind` in `missing` once.
pub(super) fn note_missing(missing: &mut Vec<MissingContrib>, kind: ContribKind, id: Arc<str>) {
    note(missing, kind, id, false);
}

/// Lists `id` in `missing` once as being checked: its settings were clamped, or it can't draw yet.
pub(super) fn note_checking(missing: &mut Vec<MissingContrib>, kind: ContribKind, id: Arc<str>) {
    note(missing, kind, id, true);
}

/// Lists `id`, which `registry` can't draw, once: as being checked when the catalogue knows it and it isn't built in (a
/// library or project copy whose check is pending, whose settings are being measured again, or which the "Agent-made
/// effects" switch leaves out), else as missing.
pub(super) fn note_not_drawn(
    registry: &Registry,
    missing: &mut Vec<MissingContrib>,
    kind: ContribKind,
    id: &ContribId,
    s: &mut Table,
) {
    let checking = !id.is_builtin() && registry.catalog().get(id).is_some();
    note(missing, kind, s.str(id.as_str()), checking);
}

fn note(missing: &mut Vec<MissingContrib>, kind: ContribKind, id: Arc<str>, checking: bool) {
    if !missing.iter().any(|m| m.id == id) {
        missing.push(MissingContrib { kind, id, checking });
    }
}

/// Stable per layer and effect, so random patterns don't change between renders.
pub fn seed_of(layer_id: &str, effect_id: &str) -> u32 {
    let h = fnv1a64(layer_id.as_bytes()) ^ fnv1a64(effect_id.as_bytes()).rotate_left(17);
    (h ^ (h >> 32)) as u32
}

/// Packs `values` into `shader`'s `Params`, each as it draws (`drawn`, the one reading the envelope's clamp and the
/// verified store share: N-m5): a missing or unusable value takes the schema's default; numbers are clamped to their
/// range, angles become radians, a choice becomes its option's index and a colour straight RGBA 0–1.
pub fn pack_params(shader: &ShaderEffect, values: &BTreeMap<String, ParamValue>) -> ParamBlock {
    let mut block = ParamBlock::zeroed(shader.layout.size);
    for &(i, _, at) in &shader.layout.fields {
        let spec = &shader.params[i];
        match drawn(spec, values.get(&spec.key)) {
            Drawn::Number(x) => match spec.kind {
                ParamKind::Int { .. } => block.put(at, &(x as i32).to_le_bytes()),
                ParamKind::Angle { .. } => block.put_f32s(at, &[x.to_radians() as f32]),
                _ => block.put_f32s(at, &[x as f32]),
            },
            Drawn::Bool(on) => block.put(at, &u32::from(on).to_le_bytes()),
            Drawn::Choice(index) => block.put(at, &index.to_le_bytes()),
            Drawn::Point(p) => block.put_f32s(at, &[p.x as f32, p.y as f32]),
            Drawn::Color(rgba) => block.put_f32s(at, &rgba),
            // `ParamLayout::of` refuses these for shaders.
            Drawn::Other(_) => {}
        }
    }
    block
}
