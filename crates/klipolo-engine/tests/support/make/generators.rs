//! Helpers for generators on effect ABI v1.1: generators that aren't built in with a
//! resolution and a frame rate, layers that are a split's piece, and frames drawn one after another on one `Headless`.

use std::collections::BTreeMap;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use klipolo_engine::plan::{FramePlan, GeneratorLayer, Layer};
use klipolo_engine::registry::{Registry, ShaderEffect, ShaderKind};
use klipolo_model::contrib::{ContribDef, ContribId, ContribKind, Resolution};
use klipolo_model::ops::Op;
use klipolo_model::project::Project;
use klipolo_model::time::Us;
use klipolo_model::timeline::{EffectInstance, ItemContent};

use super::{Rendered, render_on};

/// A generator that isn't built in, `com.test.fx/generator.<name>`, drawn at `resolution` and stepping at `fps` when
/// given.
pub fn generator_def(name: &str, resolution: Option<Resolution>, fps: Option<u32>) -> ContribDef {
    ContribDef {
        resolution,
        fps,
        ..ContribDef::new(
            ContribId::new("com.test.fx", &format!("generator.{name}")),
            ContribKind::Generator,
            name,
        )
    }
}

/// The built-ins plus each `(def, kind, body)` checked and hardened (panics on a refusal).
pub fn registry_of(shaders: &[(ContribDef, ShaderKind, &str)]) -> Arc<Registry> {
    Registry::builtin_with_test_shaders(shaders.iter().map(|(def, kind, body)| {
        let shader =
            ShaderEffect::assemble_untrusted(def, *kind, body).unwrap_or_else(|d| panic!("{d:#?}"));
        (def.clone(), shader)
    }))
}

/// A project with no main clips and one full-frame layer of the generator `def` from `start` for `seconds` (seconds),
/// under `effects`. With `piece` (offset, span in seconds) it is a split's piece: it starts `offset` into the
/// contribution's time and maps onto `span`.
pub fn generator_piece_project(
    def: &ContribDef,
    start: f64,
    seconds: f64,
    piece: Option<(f64, f64)>,
    effects: Vec<EffectInstance>,
) -> Project {
    let mut p = super::super::project();
    p.timeline.main.clear();
    let mut content = ItemContent::generator(def.id.clone(), BTreeMap::new());
    if let (
        ItemContent::Generator {
            offset_us, span_us, ..
        },
        Some((offset, span)),
    ) = (&mut content, piece)
    {
        *offset_us = super::super::secs(offset);
        *span_us = Some(super::super::secs(span));
    }
    let mut layer = super::super::item("gen", start, seconds, super::full_box(), content);
    layer.effects = effects;
    p.timeline.tracks = vec![super::super::overlay_track("g", vec![layer])];
    p
}

/// `p` with layer `id` split at `at` seconds, as the editor splits it (`Op::SplitItem`: the second piece gets an id of
/// its own).
pub fn split_at(p: &Project, id: &str, at: f64) -> Project {
    klipolo_model::apply_ops(
        p,
        &[Op::SplitItem {
            id: id.into(),
            at_us: super::super::secs(at),
        }],
    )
    .unwrap()
}

/// The first generator layer of `plan`.
pub fn generator_of(plan: &FramePlan) -> &GeneratorLayer {
    plan.layers
        .iter()
        .find_map(|l| match l {
            Layer::Generator(g) => Some(g),
            _ => None,
        })
        .expect("a generator layer")
}

/// One project's frames drawn one after another on one `Headless` (its pipelines, targets and cached generators kept
/// between them), at 1920 × 1080.
pub struct GeneratorFrames {
    headless: Headless,
    project: Arc<Project>,
}

/// `body` as a generator that isn't built in, drawn at `resolution`, filling a 5 s layer over the black background.
pub async fn headless_with_generator(body: &str, resolution: Resolution) -> GeneratorFrames {
    let def = generator_def("still", Some(resolution), None);
    frames_of(
        registry_of(&[(def.clone(), ShaderKind::Generator, body)]),
        generator_piece_project(&def, 0.0, 5.0, None, Vec::new()),
    )
    .await
}

/// `project`'s frames drawn with `registry`.
pub async fn frames_of(registry: Arc<Registry>, project: Project) -> GeneratorFrames {
    GeneratorFrames {
        headless: Headless::new(registry).await.unwrap(),
        project: Arc::new(project),
    }
}

impl GeneratorFrames {
    /// The frame at `at_us` (panics on a frame warning).
    pub async fn render_at(&mut self, at_us: Us) -> Rendered {
        self.render_sized(at_us, (1920, 1080)).await
    }

    /// The frame at `at_us`, `size` pixels (panics on a frame warning; `Rendered::rgba` rows are `size.0` wide).
    pub async fn render_sized(&mut self, at_us: Us, size: (u32, u32)) -> Rendered {
        render_on(&mut self.headless, self.project.clone(), at_us, size).await
    }

    /// Every render pass the last frame encoded (`Headless::passes_last_frame`): its one layer pass and the effect
    /// stage's.
    pub fn passes_last_frame(&self) -> u32 {
        self.headless.passes_last_frame()
    }
}
