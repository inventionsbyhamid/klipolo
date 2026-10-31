//! Helpers for scene groups (offscreen groups, masks, backdrops, generator fills, particles): a scene's frame
//! with its notes, what lies under a scene layer (a striped picture, a video whose frames change), and registries
//! with an agent-made generator (an aurora) checked here, or only catalogued.

use std::sync::Arc;

use klipolo_engine::headless::Headless;
use klipolo_engine::registry::{Registry, ShaderEffect, ShaderKind};
use klipolo_model::contrib::{ContribDef, ContribKind};
use klipolo_model::project::{Asset, AssetKind, Project};
use klipolo_model::timeline::{Clip, ClipSource, ItemContent};

use super::super::{bx, item, overlay_track, secs};
use super::{project_with_scene, scene_frame, scene_id};

/// The frame size the scene group tests draw at.
pub const G_SIZE: (u32, u32) = (640, 360);

/// The frame of scene `json` (a 2 s layer filling the frame, drawn by `registry`, or the scene's own when None) at
/// `t` seconds, and its notes.
pub async fn scene_frame_at(
    json: &str,
    t: f64,
    registry: Option<Arc<Registry>>,
) -> (image::RgbaImage, Vec<String>) {
    let (p, registry) = match registry {
        Some(r) => (project_of("g", 2.0), r),
        None => project_with_scene("g", json, 0.0, 2.0),
    };
    let mut h = Headless::new(registry).await.unwrap();
    scene_frame(&mut h, &Arc::new(p), t, G_SIZE).await
}

/// A project of one full-frame overlay layer `id` showing scene `id` (`scene_id`) for `duration` seconds, its scene
/// left to the registry the test makes.
pub fn project_of(id: &str, duration: f64) -> Project {
    let mut p = Project::new("Scene");
    p.id = "scene-project".into();
    let content = ItemContent::Generator {
        source: scene_id(id).as_str().into(),
        params: Default::default(),
        still: false,
        offset_us: 0,
        span_us: None,
        seed_from: None,
    };
    let layer = item(id, 0.0, duration, bx(0.0, 0.0, 1.0, 1.0), content);
    p.timeline.tracks.push(overlay_track("titles", vec![layer]));
    p
}

/// The local id of the aurora generator in this install's library.
pub const AURORA: &str = "lib.k3f9q2ma/generator.aurora.v1";

/// An aurora: soft bands of green and violet over deep blue that drift with time (agent-made WGSL, hardened as a
/// checked shader is).
const AURORA_WGSL: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    let t = kl.time * params.speed;
    let a = 0.5 + 0.5 * sin(uv.x * 6.0 + t * 1.3 + sin(uv.y * 3.0));
    let b = 0.5 + 0.5 * sin(uv.x * 4.0 - t * 0.7 + 2.0);
    let band = smoothstep(0.35, 0.0, abs(uv.y - 0.5 - 0.25 * (a - 0.5)));
    let c = mix(vec3<f32>(0.05, 0.1, 0.3), mix(vec3<f32>(0.1, 0.9, 0.6), vec3<f32>(0.6, 0.3, 0.9), b), band);
    return vec4<f32>(c, 1.0);
}";

/// The aurora's definition: one number setting, `speed`.
pub fn aurora_def() -> ContribDef {
    let def: ContribDef = serde_json::from_value(serde_json::json!({
        "id": AURORA, "kind": "generator", "name": "Aurora", "abi": 1,
        "params": [{"key": "speed", "label": "Speed", "type": "float", "min": 0, "max": 4, "default": 1}]
    }))
    .unwrap();
    def
}

/// The built-ins and the aurora checked here (the runnable predicate let it through), with each `(name, scene JSON)`
/// compiled against that catalogue.
pub fn aurora_registry(scenes: &[(&str, &str)]) -> Arc<Registry> {
    let def = aurora_def();
    let shader = ShaderEffect::assemble_untrusted(&def, ShaderKind::Generator, AURORA_WGSL)
        .unwrap_or_else(|d| panic!("{d:?}"));
    with_scenes(&Registry::with_checked([(def, shader)]), scenes)
}

/// The built-ins with the aurora catalogued but not checked here (it draws as nothing), and the scenes.
pub fn unchecked_aurora_registry(scenes: &[(&str, &str)]) -> Arc<Registry> {
    let base = Registry::with_definitions(&Registry::builtin(), [&aurora_def()]);
    with_scenes(&base, scenes)
}

fn with_scenes(base: &Registry, scenes: &[(&str, &str)]) -> Arc<Registry> {
    let compiled = scenes.iter().map(|(name, json)| {
        let def = ContribDef::new(scene_id(name).as_str(), ContribKind::Scene, name);
        let scene = klipolo_engine::registry::compile_scene(json.as_bytes(), base.catalog())
            .unwrap_or_else(|p| panic!("{name}: {p:#?}"));
        (def, scene)
    });
    Arc::new(base.with_scenes(compiled))
}

/// A picture of vertical stripes, 8 pixels black then 8 white, written into `dir` as image asset `stripes` and drawn
/// full frame on a track under the scene's.
pub fn stripes_under(p: &mut Project, dir: &std::path::Path) {
    let path = dir.join("stripes.png");
    image::RgbaImage::from_fn(640, 360, |x, _| {
        let v = if (x / 8) % 2 == 0 { 16 } else { 240 };
        image::Rgba([v, v, v, 255])
    })
    .save(&path)
    .unwrap();
    p.assets.push(Asset {
        id: "stripes".into(),
        kind: AssetKind::Image,
        path: path.to_string_lossy().into_owned(),
        name: "stripes.png".into(),
        duration_us: 0,
        width: 640,
        height: 360,
        fps: 0.0,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    });
    let under = item(
        "under",
        0.0,
        10.0,
        bx(0.0, 0.0, 1.0, 1.0),
        ItemContent::Image {
            asset_id: "stripes".into(),
        },
    );
    p.timeline
        .tracks
        .insert(0, overlay_track("pictures", vec![under]));
}

/// A 10 s video whose luma changes every frame (`ntsc-luma-10s.mp4`) as the main footage under the scene.
pub fn video_under(p: &mut Project) {
    p.assets.push(Asset {
        id: "luma".into(),
        kind: AssetKind::Video,
        path: super::super::fixture("ntsc-luma-10s.mp4")
            .to_string_lossy()
            .into_owned(),
        name: "luma.mp4".into(),
        duration_us: secs(10.0),
        width: 320,
        height: 180,
        fps: 29.97,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    });
    let clip = Clip::new(ClipSource::Asset { id: "luma".into() }, 0, secs(10.0));
    p.timeline.main = vec![clip];
}

/// BT.709 luma of a pixel.
pub fn luma_of(p: [u8; 3]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

/// A project of scene `json` (a layer `g`, 0–`duration` s) and its registry.
pub fn scene_project(json: &str, duration: f64) -> (Project, Arc<Registry>) {
    project_with_scene("g", json, 0.0, duration)
}

/// `img` written as golden `name` when `UPDATE_GOLDEN` is set (before a test's own checks, to look at what failed).
pub fn keep_golden(name: &str, img: &image::RgbaImage) {
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        super::scene_golden(name, img);
    }
}
