//! Drafts and projects for the render worker's tests: the waves, loop and branch bodies with
//! their settings, a draft layer and the worker's `Headless`, and the projects drawn from stills, with a contribution
//! and with text.

use std::collections::BTreeMap;

use klipolo_engine::headless::Headless;
use klipolo_engine::make::budget::RunsAt;
use klipolo_engine::make::draft::DraftLayer;
use klipolo_engine::make::envelope::Envelope;
use klipolo_engine::make::frames::ContribEntry;
use klipolo_model::contrib::{ContribDef, ContribKind};
use klipolo_model::font::FontRef;
use klipolo_model::params::ParamValue;
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::timeline::{Clip, ClipSource, ItemContent, TextStyle};

pub const WAVES_BODY: &str = include_str!("../../fixtures/wgsl/waves.wgsl");
pub const LOOP_PARAM_BODY: &str = include_str!("../../fixtures/wgsl/loop_param.wgsl");
pub const BRANCH_GATED_BODY: &str = include_str!("../../fixtures/wgsl/branch_gated.wgsl");
/// The sample sparkle filter (glints on the opaque parts of a layer).
pub const SPARKLE_BODY: &str = include_str!("../../fixtures/wgsl/sparkle.wgsl");

const WAVES_PARAMS: &str = r##"[{"key":"density","label":"D","type":"float","min":0.5,"max":8,"default":2},
 {"key":"speed","label":"S","type":"float","min":0,"max":4,"default":1},
 {"key":"deepColor","label":"Deep","type":"color","default":"#0B2A4A"},
 {"key":"shallowColor","label":"Shallow","type":"color","default":"#2F8FB8"},
 {"key":"foam","label":"F","type":"float","min":0,"max":1,"default":0.4}]"##;

/// A generator draft with the waves preset's settings.
pub fn draft_def() -> ContribDef {
    draft_def_with(WAVES_PARAMS)
}

/// A generator draft, `lib.k3f9q2ma/generator.test.v1`, with the settings `params` (JSON).
pub fn draft_def_with(params: &str) -> ContribDef {
    ContribDef {
        params: serde_json::from_str(params).unwrap(),
        ..ContribDef::new(
            "lib.k3f9q2ma/generator.test.v1",
            ContribKind::Generator,
            "Test",
        )
    }
}

/// `branch_gated.wgsl`'s generator: `detail` (a number gating a loop), `fancy` (a bool gating one) and `mode` (a
/// choice of 3 whose third option holds one).
pub fn branch_gated_def() -> ContribDef {
    draft_def_with(
        r#"[{"key":"detail","label":"D","type":"float","min":0,"max":1,"default":0},
            {"key":"fancy","label":"F","type":"bool","default":false},
            {"key":"mode","label":"M","type":"choice","options":["a","b","c"],"default":"a"}]"#,
    )
}

/// The sample sparkle filter's definition (`guard.rs` checks the same), padded by its `spacing`.
pub fn sparkle_def() -> ContribDef {
    ContribDef {
        params: serde_json::from_str(
            r#"[{"key":"spacing","label":"Spacing","type":"float","min":8,"max":120,"default":36},
                {"key":"size","label":"Size","type":"float","min":0.05,"max":1,"default":0.35},
                {"key":"speed","label":"Speed","type":"float","min":0,"max":4,"default":1},
                {"key":"amount","label":"Amount","type":"float","min":0,"max":2,"default":1}]"#,
        )
        .unwrap(),
        padding: Some(klipolo_model::contrib::PaddingSpec {
            px: 4.0,
            param: Some("spacing".into()),
            per_unit: 0.5,
            max_px: 64.0,
        }),
        ..ContribDef::new(
            "lib.k3f9q2ma/effect.sparkle.v1",
            ContribKind::Effect,
            "Sparkle",
        )
    }
}

/// `body` under `def`, checked, at its defaults (its test image in a folder for the run).
pub fn draft(def: ContribDef, body: &str) -> DraftLayer {
    static TMP: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let tmp = TMP.get_or_init(|| tempfile::tempdir().unwrap());
    DraftLayer::new(
        &def,
        body,
        BTreeMap::new(),
        FormatKind::Landscape,
        tmp.path(),
    )
    .unwrap_or_else(|d| panic!("{d:#?}"))
}

/// The worker's transparent `Headless` on a device of its own.
pub async fn headless_alone() -> Headless {
    klipolo_engine::make::gpu::worker_headless(None)
        .await
        .unwrap()
}

/// One 1920 × 1080 video, 2 s on the main track, at a path outside the worker's session (it is never opened).
pub fn video_project() -> Project {
    let mut p = Project::new("Video");
    p.assets = vec![Asset {
        id: "clip".into(),
        kind: AssetKind::Video,
        path: "/outside/the/session/clip.mp4".into(),
        name: "clip.mp4".into(),
        duration_us: 2_000_000,
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: Some(0),
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
    }];
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "clip".into() },
        0,
        2_000_000,
    )];
    p
}

/// A still of one colour, as the app decodes a video frame (at most 1024 px).
pub fn solid_still(rgb: [u8; 3]) -> image::RgbaImage {
    image::RgbaImage::from_pixel(1024, 576, image::Rgba([rgb[0], rgb[1], rgb[2], 255]))
}

const RAYS: &str = "lib.k3f9q2ma/generator.rays.v1";

fn rays_def() -> ContribDef {
    ContribDef {
        params: serde_json::from_str(
            r#"[{"key":"steps","label":"Steps","type":"int","min":1,"max":64,"default":8}]"#,
        )
        .unwrap(),
        ..ContribDef::new(RAYS, ContribKind::Generator, "Rays")
    }
}

/// The rays generator (`loop_param.wgsl`) over the whole canvas for 2 s with `steps`.
pub fn rays_project(steps: f64) -> Project {
    let mut p = Project::new("Rays");
    let params = BTreeMap::from([("steps".to_owned(), ParamValue::Number(steps))]);
    let content = ItemContent::generator(RAYS.into(), params);
    p.timeline.tracks = vec![super::super::overlay_track(
        "g",
        vec![super::super::item(
            "rays",
            0.0,
            2.0,
            super::full_box(),
            content,
        )],
    )];
    p
}

/// The rays generator as the app hands it over, checked here with `steps` bounded at `hi`.
pub fn rays_contrib_with_envelope(hi: f64) -> ContribEntry {
    ContribEntry {
        def: rays_def(),
        body: LOOP_PARAM_BODY.into(),
        runs_at: RunsAt::default(),
        envelope: Envelope {
            ranges: BTreeMap::from([("steps".to_owned(), (1.0, hi))]),
            allowed: BTreeMap::new(),
        },
        draft: false,
    }
}

/// White `text` in the bundled `family` at `weight`, 160 px, in the middle of the canvas for 2 s.
pub fn text_project(family: &str, weight: u16, text: &str) -> Project {
    let mut p = Project::new("Text");
    let content = ItemContent::Text {
        text: text.into(),
        style: TextStyle {
            font: FontRef::bundled(family),
            weight,
            size: 160.0,
            ..TextStyle::default()
        },
    };
    let at = super::super::bx(0.1, 0.3, 0.8, 0.4);
    p.timeline.tracks = vec![super::super::overlay_track(
        "t",
        vec![super::super::item("title", 0.0, 2.0, at, content)],
    )];
    p
}

/// Whether two colours are within `by` on every channel (JPEG keeps solid colours within a few levels).
pub fn near(a: [u8; 3], b: [u8; 3], by: u8) -> bool {
    a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= by)
}

/// The capped draft the Mac checks time (`make_mac`) and lavapipe checks the caps of.
/// A draft at the setting caps: two numbers costly at both ends (`c`, `d`), a choice of 9 (`mode`) and two bools (`x`,
/// `y`) ask for 14 combinations, capped at 12 with the defaults; a point (`at`) probed at its 9 positions (with `c`
/// and `d` inside their ranges), and a colour (`tint`). Its body runs `main` trips of the sweep's loop (~0.1 ms each on
/// Iris Plus, over ~0.45 ms of compositing), 2 more where `c` or `d` is at its middle (`gated_sin`: of the same loop,
/// else of plain arithmetic), and 8 of plain arithmetic for each other setting that decides work.
pub const CAP_SETTINGS: &str = r##"[{"key":"c","label":"C","type":"float","min":0,"max":1,"default":0.5},
    {"key":"d","label":"D","type":"float","min":0,"max":1,"default":0.5},
    {"key":"mode","label":"M","type":"choice","options":["a","b","c","d","e","f","g","h","i"],"default":"a"},
    {"key":"x","label":"X","type":"bool","default":false},
    {"key":"y","label":"Y","type":"bool","default":false},
    {"key":"at","label":"At","type":"point","default":{"x":0.5,"y":0.5}},
    {"key":"tint","label":"T","type":"color","default":"#808080"}]"##;

pub fn cap_body(main: u32, gated_sin: bool) -> String {
    let trip = "c += sin(f32(i) * 1.7 + uv.x * 13.0) * cos(uv.y * 7.0 + f32(i));";
    let gated = if gated_sin {
        trip
    } else {
        "c += 0.01 * f32(i);"
    };
    format!(
        "fn effect(uv: vec2<f32>) -> vec4<f32> {{
    var c = 0.0;
    for (var i = 0; i < {main}; i++) {{ {trip} }}
    if (abs(params.c - 0.5) < 0.1) {{ for (var i = 0; i < 2; i++) {{ {gated} }} }}
    if (abs(params.d - 0.5) < 0.1) {{ for (var i = 0; i < 2; i++) {{ {gated} }} }}
    switch (params.mode) {{
        case 8u: {{ for (var i = 0; i < 8; i++) {{ c += 0.01 * f32(i); }} }}
        default: {{}}
    }}
    if (params.x == 1u) {{ for (var i = 0; i < 8; i++) {{ c += 0.02 * f32(i); }} }}
    if (params.y == 1u) {{ for (var i = 0; i < 8; i++) {{ c += 0.03 * f32(i); }} }}
    if (params.at.x > 0.5) {{ for (var i = 0; i < 8; i++) {{ c += 0.04 * f32(i) * uv.x; }} }}
    if (params.tint.r > 0.9) {{ for (var i = 0; i < 8; i++) {{ c += 0.05 * f32(i) * uv.y; }} }}
    return vec4<f32>(vec3<f32>(fract(c)), 1.0);
}}"
    )
}
