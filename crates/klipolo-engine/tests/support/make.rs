//! The test helpers for shader safety, the render worker and the library
//! and in its submodules.

use std::collections::BTreeMap;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use klipolo_engine::plan::PlanContext;
use klipolo_engine::registry::{Registry, ShaderEffect, ShaderKind};
use klipolo_model::contrib::{ContribDef, ContribId, ContribKind, PaddingSpec};
use klipolo_model::layout::BoxN;
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::timeline::{EffectInstance, ItemContent};

mod drafts;
mod generators;
mod guards;
mod worker;
#[allow(unused_imports)]
pub use drafts::*;
pub use generators::*;
#[allow(unused_imports)]
pub use guards::*;
#[allow(unused_imports)]
pub use worker::*;

/// `body` as a generator that isn't built in (`ShaderEffect::assemble_untrusted`), filling a one-layer project over its
/// black background, drawn by `Headless` at `size` at 1 s: the frame's RGBA, rows packed. Panics if the shader is
/// refused or the frame comes with a warning (a layer left out would draw as the background).
pub async fn render_generator_rgba(def: &ContribDef, body: &str, size: (u32, u32)) -> Vec<u8> {
    let shader = ShaderEffect::assemble_untrusted(def, ShaderKind::Generator, body)
        .unwrap_or_else(|d| panic!("{d:#?}"));
    let registry = Registry::builtin_with_test_shader(def, shader);
    render(registry, generator_layer_project(def), size)
        .await
        .rgba
}

/// `shader` under `def` as [`render_generator_rgba`] draws it, whatever the frame warns: the frame's RGBA, rows packed,
/// and its warnings.
pub async fn render_generator_shader(
    def: &ContribDef,
    shader: ShaderEffect,
    size: (u32, u32),
) -> (Vec<u8>, Vec<String>) {
    let registry = Registry::builtin_with_test_shader(def, shader);
    let mut headless = Headless::new(registry).await.unwrap();
    let f = headless
        .render(
            Arc::new(generator_layer_project(def)),
            super::secs(1.0),
            FormatKind::Landscape,
            size,
        )
        .await
        .unwrap();
    let row = size.0 as usize * 4;
    let rgba = f
        .bytes
        .chunks(f.stride as usize)
        .take(size.1 as usize)
        .flat_map(|r| &r[..row])
        .copied()
        .collect();
    (rgba, f.warnings)
}

/// A project with no main clips and one layer of the generator `def` filling the frame for 2 s.
fn generator_layer_project(def: &ContribDef) -> Project {
    generator_piece_project(def, 0.0, 2.0, None, Vec::new())
}

/// A frame drawn by [`render`]: its RGBA, rows packed, and the effect stage's passes for it.
pub struct Rendered {
    pub rgba: Vec<u8>,
    /// `passes_last_frame()` less the frame's one layer pass (these frames have no adjustment or overlay-blended
    /// layer): the effect stage's passes.
    pub passes: u32,
}

/// One frame of `p` at 1 s on `Headless` at `size`, after `reset_effect_target_peak()`, so the peak is this frame's.
/// Panics on any frame warning (a layer left out would draw as the background).
pub async fn render(registry: Arc<Registry>, p: Project, size: (u32, u32)) -> Rendered {
    let mut headless = Headless::new(registry).await.unwrap();
    klipolo_engine::render::effects::reset_effect_target_peak();
    render_on(&mut headless, Arc::new(p), super::secs(1.0), size).await
}

/// One frame of `p` at `at_us` on `headless` (its pipelines and targets kept from the frames before), at `size`.
pub async fn render_on(
    headless: &mut Headless,
    p: Arc<Project>,
    at_us: klipolo_model::time::Us,
    size: (u32, u32),
) -> Rendered {
    let f = headless
        .render(p, at_us, FormatKind::Landscape, size)
        .await
        .unwrap();
    assert!(f.warnings.is_empty(), "{:?}", f.warnings);
    let row = size.0 as usize * 4;
    Rendered {
        rgba: f
            .bytes
            .chunks(f.stride as usize)
            .take(size.1 as usize)
            .flat_map(|r| &r[..row])
            .copied()
            .collect(),
        passes: headless
            .passes_last_frame()
            .checked_sub(1)
            .expect("a frame draws at least its layer pass"),
    }
}

/// The GPU tests of a file that read the process's `effect_target_peak` (or count GPU objects) take turns.
pub async fn gpu_turn() -> tokio::sync::MutexGuard<'static, ()> {
    static GPU: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    GPU.lock().await
}

/// The white generator `com.test.fx/generator.white`, and filters that aren't built in, each `(name, body, pad)` as
/// `com.test.fx/effect.<name>` padded by `pad` when given: the built-ins plus those.
pub fn test_registry(filters: &[(&str, &str, Option<f32>)]) -> Arc<Registry> {
    let white = ContribDef::new(WHITE, ContribKind::Generator, "White");
    let white = untrusted(
        &white,
        ShaderKind::Generator,
        "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }",
    );
    Registry::builtin_with_test_shaders(std::iter::once(white).chain(filters.iter().map(
        |&(name, body, pad)| untrusted(&padded_filter_def(name, pad), ShaderKind::Filter, body),
    )))
}

/// The white generator [`test_registry`] has.
pub const WHITE: &str = "com.test.fx/generator.white";

/// An instance of `effect` (a built-in id, or `com.test.fx/effect.<name>` for a bare name), with default settings.
pub fn fx(effect: &str) -> EffectInstance {
    let id = if effect.contains('/') {
        ContribId::from(effect)
    } else {
        ContribId::new("com.test.fx", &format!("effect.{effect}"))
    };
    EffectInstance {
        id: effect.into(),
        ..EffectInstance::new(id)
    }
}

/// A project with no main clips and one overlay layer: an image `side` px square of `rgb` (its file written in `dir`)
/// at `(x, y)` px of the 1920 × 1080 canvas, 2 s long, under `effects`.
pub fn image_project(
    dir: &std::path::Path,
    rgb: [u8; 3],
    (x, y, side): (u32, u32, u32),
    effects: Vec<EffectInstance>,
) -> Project {
    let png = dir.join(format!(
        "{:02x}{:02x}{:02x}-{side}.png",
        rgb[0], rgb[1], rgb[2]
    ));
    image::RgbaImage::from_pixel(side, side, image::Rgba([rgb[0], rgb[1], rgb[2], 255]))
        .save(&png)
        .unwrap();
    let mut p = super::project();
    p.timeline.main.clear();
    let logo = p.assets.iter_mut().find(|a| a.id == "logo").unwrap();
    logo.path = png.display().to_string();
    (logo.width, logo.height) = (side, side);
    let mut layer = super::item(
        "img",
        0.0,
        2.0,
        bx_px(x, y, side, side),
        ItemContent::Image {
            asset_id: "logo".into(),
        },
    );
    layer.effects = effects;
    p.timeline.tracks = vec![super::overlay_track("o", vec![layer])];
    p
}

/// A project with no main clips and one overlay layer: [`WHITE`] in `at`, its corners `radius` of its shorter side,
/// 2 s long, under `effects`.
pub fn generator_project(at: BoxN, radius: f64, effects: Vec<EffectInstance>) -> Project {
    let mut p = super::project();
    p.timeline.main.clear();
    let mut layer = super::item(
        "gen",
        0.0,
        2.0,
        at,
        ItemContent::generator(WHITE.into(), BTreeMap::new()),
    );
    layer.layout.radius = radius;
    layer.effects = effects;
    p.timeline.tracks = vec![super::overlay_track("g", vec![layer])];
    p
}

/// A filter that isn't built in, `com.test.fx/effect.<name>`, padded by `pad` 1080p pixels (at most 64) when given.
pub fn padded_filter_def(name: &str, pad: Option<f32>) -> ContribDef {
    ContribDef {
        padding: pad.map(|px| PaddingSpec {
            px,
            param: None,
            per_unit: 0.0,
            max_px: 64.0,
        }),
        ..ContribDef::new(
            ContribId::new("com.test.fx", &format!("effect.{name}")),
            ContribKind::Effect,
            name,
        )
    }
}

/// `def`'s shader checked and hardened from `body` (panics on a refusal).
fn untrusted(def: &ContribDef, kind: ShaderKind, body: &str) -> (ContribDef, ShaderEffect) {
    let shader =
        ShaderEffect::assemble_untrusted(def, kind, body).unwrap_or_else(|d| panic!("{d:#?}"));
    (def.clone(), shader)
}

/// One instance of each filter in `defs`, in order.
fn stack(defs: &[ContribDef]) -> Vec<EffectInstance> {
    defs.iter()
        .map(|d| EffectInstance {
            id: d.name.clone(),
            ..EffectInstance::new(d.id.clone())
        })
        .collect()
}

const PASS_THROUGH: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return kl_sample_a(uv); }";

/// An image layer (the 512 px logo in a 100 × 100 px box at (400, 300) of the 1920 × 1080 canvas) under one
/// pass-through filter per `(name, px)`, each padded by `px`, in a project with no main clips; and a registry with
/// those filters.
pub fn project_with_padded_filters(filters: &[(&str, f32)]) -> (Project, Arc<Registry>) {
    let defs: Vec<ContribDef> = filters
        .iter()
        .map(|&(name, px)| padded_filter_def(name, Some(px)))
        .collect();
    let registry = Registry::builtin_with_test_shaders(
        defs.iter()
            .map(|d| untrusted(d, ShaderKind::Filter, PASS_THROUGH)),
    );
    let mut p = super::project();
    p.timeline.main.clear();
    let mut layer = super::item(
        "img",
        0.0,
        2.0,
        bx_px(400, 300, 100, 100),
        ItemContent::Image {
            asset_id: "logo".into(),
        },
    );
    layer.effects = stack(&defs);
    p.timeline.tracks = vec![super::overlay_track("o", vec![layer])];
    (p, registry)
}

/// A plan context for `p` with `registry`.
pub fn ctx_with(p: Project, registry: Arc<Registry>) -> PlanContext {
    PlanContext::with_registry(Arc::new(p), registry)
}

/// The whole canvas.
pub fn full_box() -> BoxN {
    super::bx(0.0, 0.0, 1.0, 1.0)
}

/// A box given in pixels of the 1920 × 1080 canvas.
pub fn bx_px(x: u32, y: u32, w: u32, h: u32) -> BoxN {
    let (cw, ch) = (1920.0, 1080.0);
    super::bx(
        f64::from(x) / cw,
        f64::from(y) / ch,
        f64::from(w) / cw,
        f64::from(h) / ch,
    )
}

/// One pixel's RGB from a packed 1920-wide RGBA frame.
pub fn pixel_at(rgba: &[u8], (x, y): (u32, u32)) -> [u8; 3] {
    let i = (y as usize * 1920 + x as usize) * 4;
    [rgba[i], rgba[i + 1], rgba[i + 2]]
}

/// A 100 × 100 px red image at (400, 300) under the filter `body` (`com.test.fx/effect.halo`, padded by `pad` when
/// given), drawn at 1920 × 1080 over black: the frame's RGBA, rows packed.
pub async fn render_image_with_filter(body: &str, pad: Option<f32>) -> Vec<u8> {
    let registry = test_registry(&[("halo", body, pad)]);
    let dir = tempfile::tempdir().unwrap();
    let p = image_project(dir.path(), [255, 0, 0], (400, 300, 100), vec![fx("halo")]);
    render(registry, p, (1920, 1080)).await.rgba
}

/// A white generator that isn't built in, in `at`, under the filter `body` (`com.test.fx/effect.halo`, padded by `pad`
/// when given), drawn at 1920 × 1080 over black.
pub async fn render_generator_with_filter(body: &str, pad: Option<f32>, at: BoxN) -> Rendered {
    let registry = test_registry(&[("halo", body, pad)]);
    render(
        registry,
        generator_project(at, 0.0, vec![fx("halo")]),
        (1920, 1080),
    )
    .await
}
