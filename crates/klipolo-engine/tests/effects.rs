mod support;

use std::collections::BTreeMap;

use klipolo_engine::plan::{Layer, frame_plan, pack_params, seed_of};
use klipolo_engine::registry::{Registry, ShaderEffect, ShaderKind, validate_wgsl};
use klipolo_model::contrib::{ContribDef, ContribKind, builtin};
use klipolo_model::params::{ParamKind, ParamSpec, ParamValue, PointN};
use klipolo_model::project::FormatKind::Landscape;
use klipolo_model::timeline::{EffectInstance, ItemContent};
use support::*;

fn def(id: &str, params: Vec<ParamSpec>) -> ContribDef {
    ContribDef {
        params,
        ..ContribDef::new(id, ContribKind::Effect, "Test")
    }
}

fn radius(r: f64) -> BTreeMap<String, ParamValue> {
    BTreeMap::from([("radius".into(), ParamValue::Number(r))])
}

fn spec(key: &str, kind: ParamKind) -> ParamSpec {
    ParamSpec {
        key: key.into(),
        label: key.into(),
        visible_when: None,
        kind,
    }
}

fn effect(id: &str, contrib: &str) -> EffectInstance {
    EffectInstance {
        id: id.into(),
        ..EffectInstance::new(contrib)
    }
}

#[test]
fn builtin_shaders_assemble_and_validate() {
    let registry = Registry::builtin();
    let mut ids: Vec<&str> = registry.shaders().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            builtin::EFFECT_BLUR,
            "klipolo.builtin/effect.drop-shadow",
            "klipolo.builtin/effect.glow",
            builtin::EFFECT_MASK,
            builtin::EFFECT_PIXELATE,
            "klipolo.builtin/effect.sparkle",
            builtin::EFFECT_VIGNETTE,
            "klipolo.builtin/generator.aurora",
            "klipolo.builtin/generator.dust",
            builtin::GENERATOR_GRADIENT,
            "klipolo.builtin/generator.grain",
            "klipolo.builtin/generator.light-leak",
            "klipolo.builtin/generator.waves",
            builtin::TRANSITION_DIP,
            builtin::TRANSITION_DISSOLVE,
            builtin::TRANSITION_PUSH,
            builtin::TRANSITION_WIPE,
        ]
    );
    for shader in registry.shaders() {
        validate_wgsl(&shader.source).unwrap_or_else(|e| panic!("{}:\n{e}", shader.id));
        if let Some(then) = &shader.then {
            validate_wgsl(&then.source)
                .unwrap_or_else(|e| panic!("{} (second pass):\n{e}", shader.id));
        }
        let kind = registry.catalog().get(&shader.id).map(|d| d.kind);
        let want = match shader.kind {
            ShaderKind::Filter => ContribKind::Effect,
            ShaderKind::Generator => ContribKind::Generator,
            ShaderKind::Transition => ContribKind::Transition,
        };
        assert_eq!(kind, Some(want));
    }
    let broken = ShaderEffect::assemble(
        &def("com.test.fx/broken", vec![]),
        ShaderKind::Filter,
        "fn effect(uv: vec2<f32>) -> vec4<f32> { return nope; }",
    )
    .unwrap();
    assert!(
        validate_wgsl(&broken.source).unwrap_err().contains("nope"),
        "errors name what is wrong"
    );
}

/// `mask.wgsl` picks the shape by number (`params.shape == 0u` circle, `2u` square, `3u` frame), and a saved project
/// stores the name: reordering `MASK_SHAPES` would turn every saved shape into another.
#[test]
fn mask_shapes_keep_the_numbers_its_shader_uses() {
    assert_eq!(
        builtin::MASK_SHAPES,
        ["circle", "rounded", "square", "frame"]
    );
    let registry = Registry::builtin();
    let mask = registry.shader(&builtin::EFFECT_MASK.into()).unwrap();
    for (n, shape) in builtin::MASK_SHAPES.iter().enumerate() {
        let values = BTreeMap::from([("shape".to_string(), ParamValue::Text((*shape).into()))]);
        let block = pack_params(mask, &values);
        assert_eq!(block.bytes()[..4], (n as u32).to_le_bytes(), "{shape}");
    }
    for test in [
        "params.shape == 0u",
        "params.shape == 2u",
        "params.shape == 3u",
    ] {
        assert!(
            mask.source.contains(test),
            "mask.wgsl no longer tests {test}"
        );
    }
}

#[test]
fn params_pack_in_wgsl_uniform_layout() {
    let params = vec![
        spec(
            "amount",
            ParamKind::Float {
                min: 0.0,
                max: 1.0,
                default: 0.5,
                step: None,
                unit: None,
            },
        ),
        spec(
            "tint",
            ParamKind::Color {
                default: "#FF000080".into(),
                alpha: true,
            },
        ),
        spec(
            "centre",
            ParamKind::Point {
                default: PointN { x: 0.5, y: 0.25 },
            },
        ),
        spec(
            "mode",
            ParamKind::Choice {
                options: vec!["soft".into(), "hard".into()],
                default: "soft".into(),
            },
        ),
        spec("turn", ParamKind::Angle { default: 90.0 }),
    ];
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> { return params.tint * params.amount; }";
    let shader =
        ShaderEffect::assemble(&def("com.test.fx/packed", params), ShaderKind::Filter, body)
            .unwrap();
    validate_wgsl(&shader.source).unwrap();
    let offsets: Vec<usize> = shader.layout.fields.iter().map(|f| f.2).collect();
    assert_eq!((offsets, shader.layout.size), (vec![0, 16, 32, 40, 44], 48));

    let values = BTreeMap::from([
        ("amount".to_string(), ParamValue::Number(3.0)),
        ("mode".to_string(), ParamValue::Text("hard".into())),
    ]);
    let block = pack_params(&shader, &values);
    let bytes = block.bytes();
    let f = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let u = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    assert_eq!(bytes.len(), 48);
    assert_eq!(f(0), 1.0, "clamped to the schema");
    assert_eq!([f(16), f(20), f(24)], [1.0, 0.0, 0.0]);
    assert!(
        (f(28) - 128.0 / 255.0).abs() < 1e-6,
        "straight alpha from the default colour"
    );
    assert_eq!([f(32), f(36)], [0.5, 0.25]);
    assert_eq!(u(40), 1, "a choice is its option's index");
    assert!(
        (f(44) - std::f32::consts::FRAC_PI_2).abs() < 1e-6,
        "degrees become radians"
    );
}

#[test]
fn effect_stacks_become_ordered_passes() {
    let mut p = project();
    let mut off = effect("v", builtin::EFFECT_VIGNETTE);
    off.enabled = false;
    let blur = effect("b", builtin::EFFECT_BLUR).with("radius", ParamValue::Number(20.0));
    p.timeline.main[0].effects = vec![blur, off, effect("g", "com.acme.neon/glow")];
    let c = ctx(p);
    let plan = frame_plan(&c, secs(5.0), Landscape);
    let main = videos(&plan)[0];
    let shaders: Vec<&str> = main.effects.iter().map(|e| e.shader.id.as_str()).collect();
    assert_eq!(
        shaders,
        [builtin::EFFECT_BLUR],
        "disabled effects are skipped; missing ones too"
    );
    assert!(
        videos(&plan)[1].effects.is_empty(),
        "clip effects reach the main footage, not the camera"
    );
    assert_eq!(
        plan.missing.iter().map(|m| &*m.id).collect::<Vec<_>>(),
        ["com.acme.neon/glow"]
    );
    let pass = &main.effects[0];
    assert_eq!(
        (pass.time_s, pass.progress, pass.seed),
        (5.0, 0.25, seed_of("a", "b"))
    );
    assert_eq!(
        frame_plan(&c, secs(5.0), Landscape),
        plan,
        "the plan is pure"
    );
    assert!(
        frame_plan(&ctx(project()), secs(5.0), Landscape)
            .missing
            .is_empty()
    );
}

#[test]
fn generators_and_adjustments_are_planned_in_track_order() {
    let mut p = project();
    let mut adjust = item(
        "adj",
        1.0,
        3.0,
        bx(0.0, 0.5, 1.0, 0.5),
        ItemContent::Adjustment,
    );
    adjust.effects = vec![effect("px", builtin::EFFECT_PIXELATE)];
    let gradient = ItemContent::generator(builtin::GENERATOR_GRADIENT.into(), BTreeMap::new());
    p.timeline.tracks = vec![
        overlay_track(
            "below",
            vec![item("grad", 1.0, 3.0, bx(0.1, 0.1, 0.5, 0.5), gradient)],
        ),
        overlay_track(
            "above",
            vec![
                adjust,
                item(
                    "bare",
                    5.0,
                    1.0,
                    bx(0.0, 0.0, 1.0, 1.0),
                    ItemContent::Adjustment,
                ),
            ],
        ),
    ];
    let c = ctx(p);
    let plan = frame_plan(&c, secs(2.0), Landscape);
    let kinds: Vec<&str> = plan
        .layers
        .iter()
        .map(|l| match l {
            Layer::Video(_) => "video",
            Layer::Generator(_) => "generator",
            Layer::Adjustment(_) => "adjustment",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["video", "video", "generator", "adjustment"]);
    let Layer::Generator(g) = &plan.layers[2] else {
        unreachable!()
    };
    assert_eq!(g.source.shader.id.as_str(), builtin::GENERATOR_GRADIENT);
    // The gradient reads no time (ABI v1.1): it sees none, and draws once.
    assert_eq!((g.source.time_s, g.source.progress), (0.0, 0.0));
    assert!(g.cache_key.is_some() && !g.half);
    assert_eq!(g.source.duration_s, 3.0);
    let Layer::Adjustment(a) = &plan.layers[3] else {
        unreachable!()
    };
    close(a.dest, bx(0.0, 0.5, 1.0, 0.5));
    assert!(
        frame_plan(&c, secs(5.5), Landscape)
            .layers
            .iter()
            .all(|l| !matches!(l, Layer::Adjustment(_))),
        "an adjustment without effects draws nothing"
    );
}

#[test]
fn looks_resolve_through_the_registry() {
    let mut p = project();
    p.timeline.main[0].look.preset = Some(builtin::look("warm"));
    p.timeline.main[1].look.preset = Some("com.acme.film/look.kodak".into());
    let plan = frame_plan(&ctx(p.clone()), secs(2.0), Landscape);
    let look = videos(&plan)[0].look.as_ref().unwrap();
    assert_eq!(
        look.look.as_ref().map(|(id, g)| (&**id, g.temperature)),
        Some(("klipolo.builtin/look.warm", 0.35))
    );
    let later = frame_plan(&ctx(p), secs(25.0), Landscape);
    assert!(
        videos(&later)[0].look.is_none(),
        "a look that isn't installed draws as no look"
    );
    assert_eq!(later.missing.len(), 1);
}

/// The GPU tests here run one at a time: the first counts the GPU objects every effect stage in the process makes,
/// and the second times its renders.
static GPU: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// One pixel's RGB from a tightly or padded RGBA frame.
fn pixel(bytes: &[u8], stride: u32, x: u32, y: u32) -> [u8; 3] {
    let i = (y * stride + x * 4) as usize;
    [bytes[i], bytes[i + 1], bytes[i + 2]]
}

// The one GPU test of this task (in this file rather than `gpu.rs`, which another task was editing at the time).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn generators_effects_and_adjustments_draw() {
    use std::sync::Arc;

    use klipolo_engine::headless::Headless;
    use klipolo_engine::render::effects::gpu_objects_made;
    use klipolo_model::project::FormatKind;

    let _gpu = GPU.lock().await;
    // A stage compiles each pipeline on first use; `warm` (the GPU warm-up's, on a device of its own) compiles
    // every built-in and internal one, so the engine's then come from Metal's shader cache.
    let before = gpu_objects_made();
    {
        let dev = klipolo_engine::render::device::RenderDevice::new()
            .await
            .unwrap();
        let mut stage = klipolo_engine::render::effects::EffectStage::new(
            &dev,
            klipolo_engine::render::mode::RenderMode::Contained,
        );
        assert_eq!(gpu_objects_made(), before, "a new stage compiles nothing");
        stage.warm();
    }
    assert_eq!(
        gpu_objects_made() - before,
        14 + klipolo_engine::registry::presets::SHADER_BODIES.len() as u64,
        "grab, resample, overlay, pad, blur ×2, mask, pixelate, vignette, gradient, dissolve, dip, push, wipe, and each \
         shader preset"
    );
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();
    let mut render = async |p: klipolo_model::project::Project| {
        headless
            .render(Arc::new(p), secs(1.0), FormatKind::Landscape, (640, 360))
            .await
            .unwrap()
    };

    let mut p = project();
    p.timeline.main.clear();
    let gradient = ItemContent::generator(
        builtin::GENERATOR_GRADIENT.into(),
        BTreeMap::from([
            ("startColor".into(), ParamValue::Text("#000000".into())),
            ("endColor".into(), ParamValue::Text("#FFFFFF".into())),
            ("angle".into(), ParamValue::Number(0.0)),
        ]),
    );
    let mut cells = item(
        "px",
        0.0,
        2.0,
        bx(0.5, 0.0, 0.5, 1.0),
        ItemContent::Adjustment,
    );
    cells.effects = vec![EffectInstance {
        id: "p".into(),
        ..EffectInstance::new(builtin::EFFECT_PIXELATE).with("size", ParamValue::Number(96.0))
    }];
    p.timeline.tracks = vec![
        overlay_track(
            "g",
            vec![item("grad", 0.0, 2.0, bx(0.0, 0.0, 1.0, 1.0), gradient)],
        ),
        overlay_track("a", vec![cells]),
    ];
    let f = render(p).await;
    let px = |x: u32| pixel(&f.bytes, f.stride, x, 180)[0];
    assert!(
        px(5) < 40 && px(5) < px(150) && px(150) < px(310) && px(310) > 160,
        "left to right: {} {} {}",
        px(5),
        px(150),
        px(310)
    );
    // 96 px cells at 1080p are 32 px at 360p, counted from the adjustment's left edge (x = 320).
    assert_eq!(px(321), px(350), "one cell is one colour");
    assert_ne!(px(30), px(60), "the left half isn't pixelated");

    // Blur (across, then down): a white top-left quadrant on black, blurred by 24 px at 1080p (8 px at 360p), ramps
    // smoothly over both of its edges.
    let mut q = project();
    q.timeline.main.clear();
    let white = ItemContent::generator(
        builtin::GENERATOR_GRADIENT.into(),
        BTreeMap::from([
            ("startColor".into(), ParamValue::Text("#FFFFFF".into())),
            ("endColor".into(), ParamValue::Text("#FFFFFF".into())),
        ]),
    );
    let mut soft = item(
        "soft",
        0.0,
        2.0,
        bx(0.0, 0.0, 1.0, 1.0),
        ItemContent::Adjustment,
    );
    soft.effects = vec![EffectInstance {
        id: "b".into(),
        ..EffectInstance::new(builtin::EFFECT_BLUR).with("radius", ParamValue::Number(24.0))
    }];
    q.timeline.tracks = vec![
        overlay_track(
            "w",
            vec![item("white", 0.0, 2.0, bx(0.0, 0.0, 0.5, 0.5), white)],
        ),
        overlay_track("s", vec![soft]),
    ];
    let f = render(q).await;
    let at = |x: u32, y: u32| pixel(&f.bytes, f.stride, x, y)[0];
    for (name, ramp) in [
        (
            "across",
            [
                at(296, 90),
                at(316, 90),
                at(320, 90),
                at(324, 90),
                at(344, 90),
            ],
        ),
        (
            "down",
            [
                at(100, 156),
                at(100, 176),
                at(100, 180),
                at(100, 184),
                at(100, 204),
            ],
        ),
    ] {
        assert!(
            ramp[0] > 245
                && ramp[0] >= ramp[1]
                && ramp[1] > ramp[2]
                && ramp[2] > ramp[3]
                && ramp[4] < 10,
            "{name}: {ramp:?}"
        );
        assert!(
            (60..200).contains(&ramp[2]),
            "{name}: the edge is half way: {ramp:?}"
        );
    }

    // An image decoded bigger than it's drawn goes through its mipmaps before its effects: fine black and white
    // stripes drawn small through an identity effect (a vignette of 0) read as an even grey, not as moiré.
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("stripes.png");
    image::RgbaImage::from_fn(1024, 1024, |x, _| {
        let v = if (x / 4) % 2 == 0 { 0 } else { 255 };
        image::Rgba([v, v, v, 255])
    })
    .save(&png)
    .unwrap();
    let mut r = project();
    r.timeline.main.clear();
    let logo = r.assets.iter_mut().find(|a| a.id == "logo").unwrap();
    (logo.path, logo.width, logo.height) = (png.display().to_string(), 1024, 1024);
    let mut stripes = item(
        "stripes",
        0.0,
        2.0,
        bx(0.1, 0.1, 100.0 / 640.0, 100.0 / 360.0),
        ItemContent::Image {
            asset_id: "logo".into(),
        },
    );
    stripes.effects = vec![EffectInstance {
        id: "v".into(),
        ..EffectInstance::new(builtin::EFFECT_VIGNETTE).with("amount", ParamValue::Number(0.0))
    }];
    r.timeline.tracks = vec![overlay_track("i", vec![stripes])];
    let f = render(r).await;
    let row: Vec<u8> = (74..154)
        .map(|x| pixel(&f.bytes, f.stride, x, 86)[0])
        .collect();
    assert!(row.iter().all(|v| (70..=185).contains(v)), "{row:?}");
}

/// A solid white generator layer with `effects`, in `rect`, over a full-frame blue one.
fn white_layer(
    rect: klipolo_model::layout::BoxN,
    effects: Vec<EffectInstance>,
) -> klipolo_model::project::Project {
    let mut p = project();
    p.timeline.main.clear();
    let solid = |colour: &str| {
        ItemContent::generator(
            builtin::GENERATOR_GRADIENT.into(),
            BTreeMap::from([
                ("startColor".into(), ParamValue::Text(colour.into())),
                ("endColor".into(), ParamValue::Text(colour.into())),
            ]),
        )
    };
    let mut layer = item("white", 0.0, 2.0, rect, solid("#FFFFFF"));
    layer.effects = effects;
    let blue = item("blue", 0.0, 2.0, bx(0.0, 0.0, 1.0, 1.0), solid("#0000FF"));
    p.timeline.tracks = vec![
        overlay_track("b", vec![blue]),
        overlay_track("w", vec![layer]),
    ];
    p
}

fn mask(shape: &str) -> EffectInstance {
    effect("m", builtin::EFFECT_MASK).with("shape", ParamValue::Text(shape.into()))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_shape_mask_cuts_the_layer_to_its_shape() {
    use std::sync::Arc;
    use std::time::Instant;

    use klipolo_engine::headless::Headless;
    use klipolo_engine::render::effects::gpu_objects_made;
    use klipolo_model::project::{FormatKind, Project};

    let _gpu = GPU.lock().await;
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();
    // A white layer in a 180 px square (x 230–410, y 90–270 at 640×360) over blue, so a pixel's red reads the
    // mask's coverage and what it clears shows the layer below. The default feather (1 px at 1080p) is a third of a
    // pixel here.
    let square = bx(230.0 / 640.0, 0.25, 180.0 / 640.0, 0.5);
    let mut render = async |rect, effects: Vec<EffectInstance>| {
        let f = headless
            .render(
                Arc::new(white_layer(rect, effects)),
                secs(1.0),
                FormatKind::Landscape,
                (640, 360),
            )
            .await
            .unwrap();
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
        move |x: u32, y: u32| pixel(&f.bytes, f.stride, x, y)
    };
    // 3 px in from each corner of the box; 2 px in from the middle of each side; well inside the top-left corner.
    let corners = [(233, 93), (406, 93), (233, 266), (406, 266)];
    let sides = [(232, 180), (407, 180), (320, 92), (320, 267)];
    let in_corner = (245, 105);

    let at = render(square, vec![mask("circle")]).await;
    let (centre, corner, side) = (
        at(320, 180)[0],
        corners.map(|(x, y)| at(x, y)[0]),
        sides.map(|(x, y)| at(x, y)[0]),
    );
    eprintln!(
        "circle: centre {centre}, corners {corner:?}, sides {side:?}, in corner {}",
        at(245, 105)[0]
    );
    assert!(centre > 250, "opaque at the centre: {centre}");
    assert!(
        corner.iter().all(|&v| v < 5),
        "clear in the corners: {corner:?}"
    );
    assert!(
        side.iter().all(|&v| v > 250),
        "opaque at the sides: {side:?}"
    );
    assert!(
        at(in_corner.0, in_corner.1)[0] < 5,
        "a circle cuts the corner deeply"
    );
    assert!(
        corners.iter().all(|&(x, y)| at(x, y)[2] > 250),
        "the layer below shows where the mask clears"
    );

    // 20 % of 180 px: corners of radius 36 px.
    let at = render(square, vec![mask("rounded")]).await;
    let (corner, side) = (
        corners.map(|(x, y)| at(x, y)[0]),
        sides.map(|(x, y)| at(x, y)[0]),
    );
    eprintln!(
        "rounded: corners {corner:?}, sides {side:?}, in corner {}",
        at(245, 105)[0]
    );
    assert!(
        corner.iter().all(|&v| v < 5),
        "clear in the corners: {corner:?}"
    );
    assert!(
        side.iter().all(|&v| v > 250),
        "opaque at the sides: {side:?}"
    );
    assert!(
        at(in_corner.0, in_corner.1)[0] > 250,
        "inside the corner's arc"
    );

    let at = render(square, vec![mask("square")]).await;
    let corner = corners.map(|(x, y)| at(x, y)[0]);
    eprintln!(
        "square: corners {corner:?}, the box's corner pixel {}",
        at(230, 90)[0]
    );
    assert!(
        corner.iter().all(|&v| v > 250),
        "opaque in the corners: {corner:?}"
    );

    // A 12 px border at 1080p is 4 px here, inside the edge.
    let at = render(
        square,
        vec![
            mask("circle")
                .with("borderWidth", ParamValue::Number(12.0))
                .with("borderColor", ParamValue::Text("#FF0000".into())),
        ],
    )
    .await;
    let (edge, deeper, centre) = (at(232, 180), at(238, 180), at(320, 180));
    eprintln!("border: 2 px in {edge:?}, 8 px in {deeper:?}, centre {centre:?}");
    assert!(
        edge[0] > 245 && edge[1] < 10 && edge[2] < 10,
        "the border's colour just inside the edge: {edge:?}"
    );
    assert!(
        deeper.iter().all(|&v| v > 245),
        "the layer inside the border: {deeper:?}"
    );
    assert!(centre.iter().all(|&v| v > 245), "{centre:?}");
    assert!(at(233, 93)[0] < 5, "the border stays inside the shape");

    // A 16:9 layer (320×180, x 160–480): the shapes are as big as its shorter side, so the circle is a true circle
    // of radius 90 in the middle, not an ellipse.
    let wide = bx(0.25, 0.25, 0.5, 0.5);
    let at = render(wide, vec![mask("circle")]).await;
    let opaque =
        [(320, 180), (232, 180), (407, 180), (320, 92), (320, 267)].map(|(x, y)| at(x, y)[0]);
    let clear = [(170, 180), (226, 180), (413, 180), (470, 180)].map(|(x, y)| at(x, y)[0]);
    eprintln!("16:9 circle: inside {opaque:?}, outside {clear:?}");
    assert!(
        opaque.iter().all(|&v| v > 250),
        "inside the circle: {opaque:?}"
    );
    assert!(
        clear.iter().all(|&v| v < 5),
        "left and right of it: {clear:?}"
    );

    // `center` spans the circle's travel: on the longer side 0–1 runs from touching the left edge (centre x 250) to
    // touching the right one (390), so a quarter puts its centre at x 285 (it spans 195–375); the shorter side has
    // no travel, so it stays centred vertically.
    let at = render(
        wide,
        vec![mask("circle").with("center", ParamValue::Point(PointN { x: 0.25, y: 0.0 }))],
    )
    .await;
    let opaque = [(200, 180), (370, 180), (285, 93), (285, 267)].map(|(x, y)| at(x, y)[0]);
    let clear = [(190, 180), (380, 180)].map(|(x, y)| at(x, y)[0]);
    eprintln!("16:9 circle a quarter across: inside {opaque:?}, outside {clear:?}");
    assert!(
        opaque.iter().all(|&v| v > 250),
        "a quarter of the travel: {opaque:?}"
    );
    assert!(clear.iter().all(|&v| v < 5), "beyond it: {clear:?}");

    // All the way: asked for the top right corner, it touches the right edge (centre x 390), still centred
    // vertically.
    let at = render(
        wide,
        vec![mask("circle").with("center", ParamValue::Point(PointN { x: 1.0, y: 0.0 }))],
    )
    .await;
    let opaque =
        [(390, 180), (305, 180), (476, 180), (390, 92), (390, 267)].map(|(x, y)| at(x, y)[0]);
    let clear = [(170, 180), (296, 180)].map(|(x, y)| at(x, y)[0]);
    eprintln!("16:9 circle moved right: inside {opaque:?}, outside {clear:?}");
    assert!(
        opaque.iter().all(|&v| v > 250),
        "the moved circle, whole: {opaque:?}"
    );
    assert!(clear.iter().all(|&v| v < 5), "where it was: {clear:?}");

    // `frame`: the whole layer with corners of 20 % of its shorter side (36 px).
    let at = render(wide, vec![mask("frame")]).await;
    let corner = [(163, 93), (476, 93), (163, 266), (476, 266)].map(|(x, y)| at(x, y)[0]);
    let side =
        [(162, 180), (477, 180), (320, 92), (320, 267), (175, 105)].map(|(x, y)| at(x, y)[0]);
    eprintln!("16:9 frame: corners {corner:?}, sides and inside the arc {side:?}");
    assert!(
        corner.iter().all(|&v| v < 5),
        "clear in the corners: {corner:?}"
    );
    assert!(
        side.iter().all(|&v| v > 250),
        "opaque at the sides: {side:?}"
    );

    // Memory: warm, a masked layer makes no GPU objects (its targets, uniform slot and pipeline are reused), however
    // its settings change from frame to frame.
    let made = gpu_objects_made();
    for x in [0.0, 0.5, 1.0] {
        let _ = render(
            wide,
            vec![mask("circle").with("center", ParamValue::Point(PointN { x, y: 0.5 }))],
        )
        .await;
    }
    assert_eq!(
        gpu_objects_made(),
        made,
        "a warm masked frame made GPU objects"
    );

    // Cost at 1280×720 with a full-frame layer (bigger than any facecam): renders without effects, with one mask,
    // with eight masks stacked (to read one pass above the noise) and, for scale, with eight identity vignettes, the
    // lightest pass there is. The four are interleaved; each render includes its readback, so the differences are
    // the passes.
    let stack = |n: usize, one: &dyn Fn() -> EffectInstance| {
        let effects = (0..n)
            .map(|i| EffectInstance {
                id: format!("e{i}"),
                ..one()
            })
            .collect();
        Arc::new(white_layer(bx(0.0, 0.0, 1.0, 1.0), effects))
    };
    let rounded = || mask("rounded").with("borderWidth", ParamValue::Number(6.0));
    let identity =
        || EffectInstance::new(builtin::EFFECT_VIGNETTE).with("amount", ParamValue::Number(0.0));
    let projects = [
        stack(0, &rounded),
        stack(1, &rounded),
        stack(8, &rounded),
        stack(8, &identity),
    ];
    let mut ms: [Vec<f64>; 4] = Default::default();
    let mut once = async |p: &Arc<Project>| {
        let started = Instant::now();
        headless
            .render(p.clone(), secs(1.0), FormatKind::Landscape, (1280, 720))
            .await
            .unwrap();
        started.elapsed().as_secs_f64() * 1000.0
    };
    for p in &projects {
        once(p).await;
    }
    for _ in 0..60 {
        for (i, p) in projects.iter().enumerate() {
            ms[i].push(once(p).await);
        }
    }
    let [plain, one, eight, vignettes] = ms.map(|mut v| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    });
    eprintln!(
        "shape mask at 1280×720, full frame, median ms/frame: {plain:.2} without, {one:.2} with one (+{:.2}), \
         {eight:.2} with eight (+{:.3} a pass); eight identity vignettes {vignettes:.2} (+{:.3} a pass)",
        one - plain,
        (eight - plain) / 8.0,
        (vignettes - plain) / 8.0,
    );
}

#[test]
fn blur_runs_at_half_size_from_eight_output_pixels() {
    // The rule is data on the contribution: blur's definition names its setting and threshold.
    let catalog = klipolo_model::contrib::Catalog::builtin();
    let spec = catalog
        .get(&builtin::EFFECT_BLUR.into())
        .unwrap()
        .downsample
        .clone();
    assert_eq!(
        spec,
        Some(klipolo_model::contrib::DownsampleSpec {
            param: "radius".into(),
            at_least_px: 8.0
        })
    );
    let registry = Registry::builtin();
    let blur = registry.shader(&builtin::EFFECT_BLUR.into()).unwrap();
    let rule = blur.downsample.expect("built-in blur may downsample");
    assert_eq!(rule.param_offset, 0);
    // radius 12 at 720p (px = 0.667) is 8 output pixels: half size. Radius 6 isn't.
    assert!(rule.applies(&pack_params(blur, &radius(12.0)), 720.0 / 1080.0));
    assert!(!rule.applies(&pack_params(blur, &radius(6.0)), 720.0 / 1080.0));
    // Other built-in shaders don't declare it.
    assert!(
        registry
            .shader(&builtin::EFFECT_VIGNETTE.into())
            .unwrap()
            .downsample
            .is_none()
    );
}

/// A built-in may run at half size from any number setting; one that isn't built in only from a px one (ABI v1.1,
/// `tests/abi_v11.rs`).
#[test]
fn a_downsample_off_a_setting_without_a_unit_is_a_built_ins_only() {
    let soft = |id: &str| ContribDef {
        downsample: Some(klipolo_model::contrib::DownsampleSpec {
            param: "radius".into(),
            at_least_px: 8.0,
        }),
        ..def(
            id,
            vec![spec(
                "radius",
                ParamKind::Float {
                    min: 0.0,
                    max: 64.0,
                    default: 12.0,
                    step: None,
                    unit: None,
                },
            )],
        )
    };
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {\n    return kl_sample_a(uv);\n}\n";
    let error =
        ShaderEffect::assemble(&soft("com.example.fx/soft"), ShaderKind::Filter, body).unwrap_err();
    assert_eq!(
        error,
        "com.example.fx/soft: \"radius\" isn't a number setting in px, so it can't choose the size."
    );
    assert!(
        ShaderEffect::assemble(
            &soft("klipolo.builtin/effect.soft"),
            ShaderKind::Filter,
            body
        )
        .unwrap()
        .downsample
        .is_some()
    );
}

/// A shader whose settings' conditions don't hold (`ContribDef::check_conditions`) is refused when it loads.
#[test]
fn shaders_refuse_a_condition_on_a_missing_choice() {
    let mut amount = spec(
        "amount",
        ParamKind::Float {
            min: 0.0,
            max: 1.0,
            default: 0.5,
            step: None,
            unit: None,
        },
    );
    amount.visible_when = Some(klipolo_model::params::VisibleWhen {
        key: "mode".into(),
        values: vec!["soft".into()],
    });
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> {\n    return kl_sample_a(uv);\n}\n";
    let error = ShaderEffect::assemble(
        &def("com.example.fx/soft", vec![amount]),
        ShaderKind::Filter,
        body,
    )
    .unwrap_err();
    assert_eq!(
        error,
        "com.example.fx/soft: \"amount\" depends on \"mode\", which isn't a choice setting of this contribution."
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "GPU: run by hand (needs scripts/gen-engine-fixtures.sh)"]
async fn half_res_blur_matches_full_res_and_costs_less() {
    use std::sync::Arc;
    use std::time::Instant;

    use klipolo_engine::headless::Headless;
    use klipolo_model::project::{AssetKind, FormatKind, Project};
    use klipolo_model::timeline::{Clip, ClipSource};

    // Two full-frame blurs (radius 24) over the bars fixture at 1280×720, 60 frames each way:
    // - mean absolute difference from a full-resolution render (downsampling off through
    //   `Headless::set_downsample(false)`) ≤ 2/255;
    // - print the median ms per frame of both (render, GPU and readback), and of the bars alone, for the report.
    let mut bars = project();
    let screen = bars.assets.iter_mut().find(|a| a.id == "screen").unwrap();
    screen.path = fixture("bars-30fps-0.5gop.mp4").display().to_string();
    screen.duration_us = secs(4.0);
    screen.kind = AssetKind::Video;
    bars.timeline.main = vec![Clip::new(
        ClipSource::Asset {
            id: "screen".into(),
        },
        0,
        secs(4.0),
    )];
    let mut blurred = bars.clone();
    let blur = |id: &str| {
        let mut adjust = item(
            id,
            0.0,
            4.0,
            bx(0.0, 0.0, 1.0, 1.0),
            ItemContent::Adjustment,
        );
        adjust.effects =
            vec![effect("b", builtin::EFFECT_BLUR).with("radius", ParamValue::Number(24.0))];
        overlay_track(id, vec![adjust])
    };
    blurred.timeline.tracks = vec![blur("one"), blur("two")];
    let (bars, blurred) = (Arc::new(bars), Arc::new(blurred));

    let _gpu = GPU.lock().await;
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();
    // The same instant every time: the decoder serves it from its cache, so the runs differ only in their passes.
    let run = async |headless: &mut Headless, project: &Arc<Project>| {
        let mut ms = Vec::with_capacity(60);
        let mut last = None;
        for _ in 0..60 {
            let started = Instant::now();
            let frame = headless
                .render(
                    project.clone(),
                    secs(1.0),
                    FormatKind::Landscape,
                    (1280, 720),
                )
                .await
                .unwrap();
            ms.push(started.elapsed().as_secs_f64() * 1000.0);
            last = Some(frame);
        }
        ms.sort_by(f64::total_cmp);
        (ms[ms.len() / 2], last.unwrap())
    };
    let (alone, _) = run(&mut headless, &bars).await;
    let (half_ms, half) = run(&mut headless, &blurred).await;
    headless.set_downsample(false);
    let (full_ms, full) = run(&mut headless, &blurred).await;

    let (mut sum, mut max, mut n) = (0u64, 0u8, 0u64);
    for y in 0..720 {
        for x in 0..1280 {
            let (a, b) = (
                pixel(&half.bytes, half.stride, x, y),
                pixel(&full.bytes, full.stride, x, y),
            );
            for c in 0..3 {
                let d = a[c].abs_diff(b[c]);
                sum += u64::from(d);
                max = max.max(d);
                n += 1;
            }
        }
    }
    let mean = sum as f64 / n as f64;
    eprintln!(
        "half-res blur: median {half_ms:.2} ms/frame, full-res {full_ms:.2} ms/frame, bars alone {alone:.2} ms/frame \
         (two blurs: {:.2} vs {:.2} ms); mean |difference| {mean:.3}/255, max {max}/255",
        half_ms - alone,
        full_ms - alone,
    );
    assert!(mean <= 2.0, "mean absolute difference {mean:.3}/255");
}

#[test]
fn every_builtin_transition_assembles_and_validates() {
    let registry = Registry::builtin();
    for id in [
        builtin::TRANSITION_DISSOLVE,
        builtin::TRANSITION_DIP,
        builtin::TRANSITION_PUSH,
        builtin::TRANSITION_WIPE,
    ] {
        let shader = registry
            .shader(&id.into())
            .unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(shader.kind, ShaderKind::Transition);
        validate_wgsl(&shader.source).unwrap_or_else(|e| panic!("{id}: {e}"));
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "GPU: run by hand (needs scripts/gen-engine-fixtures.sh)"]
async fn a_dissolve_halfway_mixes_red_and_blue() {
    use std::sync::Arc;
    use std::time::Instant;

    use klipolo_engine::headless::Headless;
    use klipolo_engine::render::effects::gpu_objects_made;
    use klipolo_model::ops::Op;
    use klipolo_model::project::{FormatKind, Project};
    use klipolo_model::timeline::{Clip, ClipSource, Transition};

    // Two asset clips: solid-red.mp4 0–1.5 s (0.5 s spare after its out point), then solid-blue.mp4 0.5–2 s (0.5 s
    // spare before its in point), joined by a 1 s dissolve: the window is 1–2 s and the cut, at progress 0.5, 1.5 s.
    let mut p = project();
    p.timeline.tracks.clear();
    // Only the two files play; the take goes, as its runs would outlast the shortened screen file.
    p.recordings.clear();
    for (id, file) in [("screen", "solid-red.mp4"), ("broll", "solid-blue.mp4")] {
        let asset = p.assets.iter_mut().find(|a| a.id == id).unwrap();
        asset.path = fixture(file).display().to_string();
        asset.duration_us = secs(2.0);
    }
    let clip = |id: &str, asset: &str, from: f64, to: f64| Clip {
        id: id.into(),
        ..Clip::new(ClipSource::Asset { id: asset.into() }, secs(from), secs(to))
    };
    p.timeline.main = vec![clip("a", "screen", 0.0, 1.5), clip("b", "broll", 0.5, 2.0)];
    let p = klipolo_model::apply_ops(
        &p,
        &[Op::SetTransition {
            clip_id: "a".into(),
            transition: Some(Transition {
                effect: builtin::TRANSITION_DISSOLVE.into(),
                duration_us: secs(1.0),
                params: Default::default(),
            }),
        }],
    )
    .unwrap();
    let p: Arc<Project> = Arc::new(p);

    let _gpu = GPU.lock().await;
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();
    let mut render = async |at: f64, size: (u32, u32)| {
        headless
            .render(p.clone(), secs(at), FormatKind::Landscape, size)
            .await
            .unwrap()
    };
    let centre = |f: &klipolo_engine::session::RenderedRgba| {
        pixel(&f.bytes, f.stride, f.width / 2, f.height / 2)
    };
    // Each colour alone (before and after the window), then the cut.
    let red = centre(&render(0.5, (640, 360)).await);
    let blue = centre(&render(2.5, (640, 360)).await);
    let started = Instant::now();
    let cut = render(1.5, (640, 360)).await;
    let first_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert!(cut.warnings.is_empty(), "{:?}", cut.warnings);
    let mixed = centre(&cut);
    assert!(red[0] > 200 && red[1] < 40 && red[2] < 40, "red {red:?}");
    assert!(blue[2] > 180 && blue[0] < 80, "blue {blue:?}");
    for c in 0..3 {
        let want = (f32::from(red[c]) + f32::from(blue[c])) / 2.0;
        assert!(
            (f32::from(mixed[c]) - want).abs() <= 3.0,
            "halfway: {mixed:?}, red {red:?}, blue {blue:?}"
        );
    }
    // Off centre the direction shows (outgoing and incoming swapped would run the dissolve backwards): a quarter in,
    // the eased mix (0.15625) is mostly red; three quarters in, mostly blue.
    for (at, progress) in [(1.25, 0.25f32), (1.75, 0.75)] {
        let eased = progress * progress * (3.0 - 2.0 * progress);
        let got = centre(&render(at, (640, 360)).await);
        for c in 0..3 {
            let (from, to) = (f32::from(red[c]), f32::from(blue[c]));
            let want = from + (to - from) * eased;
            assert!(
                (f32::from(got[c]) - want).abs() <= 3.0,
                "{progress} in: {got:?}, channel {c} wants {want:.1} (red {red:?}, blue {blue:?})"
            );
        }
    }

    // Warm, a transition frame makes no GPU objects (targets, the pair's bind group, uniform slots all reused), at
    // any progress.
    let _ = render(1.25, (1280, 720)).await;
    let made = gpu_objects_made();
    for at in [1.25, 1.5, 1.75, 1.5] {
        let _ = render(at, (1280, 720)).await;
    }
    assert_eq!(
        gpu_objects_made(),
        made,
        "a warm transition frame made GPU objects"
    );

    // Cost at 720p: the same instant 30 times (the decoders serve it from their caches), a transition frame against
    // a plain one.
    let mut median = async |at: f64| {
        let mut ms = Vec::with_capacity(30);
        for _ in 0..30 {
            let started = Instant::now();
            let _ = render(at, (1280, 720)).await;
            ms.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        ms.sort_by(f64::total_cmp);
        ms[ms.len() / 2]
    };
    let plain = median(0.5).await;
    let transition = median(1.5).await;
    eprintln!(
        "dissolve at the cut: centre {mixed:?} (red {red:?}, blue {blue:?}); first render {first_ms:.1} ms at 640×360; \
         720p median {transition:.2} ms/frame vs {plain:.2} ms/frame for one clip (+{:.2} ms)",
        transition - plain
    );
}

/// A phone video stored turned (`phone-portrait.mov`: stored 1280×720, left half red and right half blue, shown turned
/// 90° clockwise as 720×1280, red on top and blue below) as an overlay of `rect`, with `effects`, over a full-frame
/// green layer.
fn phone_overlay(
    rect: klipolo_model::layout::BoxN,
    effects: Vec<EffectInstance>,
) -> klipolo_model::project::Project {
    let mut p = white_layer(bx(0.0, 0.0, 1.0, 1.0), Vec::new());
    let green = p.timeline.tracks[0].items[0].clone();
    let asset = p.assets.iter_mut().find(|a| a.id == "broll").unwrap();
    asset.path = fixture("phone-portrait.mov").display().to_string();
    asset.duration_us = secs(1.0);
    (asset.width, asset.height) = (720, 1280); // what the import probe stores: the shown size
    let mut phone = item(
        "phone",
        0.0,
        1.0,
        rect,
        ItemContent::Video {
            asset_id: "broll".into(),
            source_in_us: 0,
            speed: 1.0,
            volume: 1.0,
            muted: true,
            mirror: false,
            tie: None,
        },
    );
    phone.effects = effects;
    let mut under = green;
    under.content = ItemContent::generator(
        builtin::GENERATOR_GRADIENT.into(),
        BTreeMap::from([
            ("startColor".into(), ParamValue::Text("#00FF00".into())),
            ("endColor".into(), ParamValue::Text("#00FF00".into())),
        ]),
    );
    under.duration_us = secs(1.0);
    p.timeline.tracks = vec![
        overlay_track("g", vec![under]),
        overlay_track("v", vec![phone]),
    ];
    p
}

/// Effects on an overlay video stored turned run on the picture as it's shown, so a mask's
/// `center` moves along the screen's axes. Also measures the upright pass.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "GPU: run by hand (needs scripts/gen-engine-fixtures.sh)"]
async fn effects_on_a_turned_phone_video_run_upright() {
    use std::sync::Arc;
    use std::time::Instant;

    use klipolo_engine::headless::Headless;
    use klipolo_engine::render::effects::gpu_objects_made;
    use klipolo_model::project::{FormatKind, Project};

    let _gpu = GPU.lock().await;
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();
    // In a 320×360 box at 640×360 the 9:16 picture is drawn 202.5×360 (x 218.75–421.25): the circle is 202.5 across,
    // can't move sideways and travels 157.5 px up and down.
    let rect = bx(0.25, 0.0, 0.5, 1.0);
    let mut render = async |effects: Vec<EffectInstance>| {
        let f = headless
            .render(
                Arc::new(phone_overlay(rect, effects)),
                secs(0.5),
                FormatKind::Landscape,
                (640, 360),
            )
            .await
            .unwrap();
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
        move |x: u32, y: u32| pixel(&f.bytes, f.stride, x, y)
    };
    let red = |c: [u8; 3]| c[0] > 200 && c[1] < 60 && c[2] < 60;
    let blue = |c: [u8; 3]| c[2] > 200 && c[0] < 60 && c[1] < 60;
    let green = |c: [u8; 3]| c[1] > 200 && c[0] < 60 && c[2] < 60;
    let circle = |x: f64, y: f64| mask("circle").with("center", ParamValue::Point(PointN { x, y }));

    // Without effects the picture is upright: red on top, blue below, green beside it.
    let at = render(Vec::new()).await;
    let plain = [at(320, 60), at(320, 300), at(200, 180)];
    eprintln!(
        "turned phone, no effects: top {:?}, bottom {:?}, beside {:?}",
        plain[0], plain[1], plain[2]
    );
    assert!(
        red(plain[0]) && blue(plain[1]) && green(plain[2]),
        "{plain:?}"
    );

    // `center` at the bottom: the circle (y 157.5–360) sits on the blue half and the top clears to green.
    let at = render(vec![circle(0.5, 1.0)]).await;
    let bottom = [at(320, 100), at(320, 170), at(320, 300), at(240, 300)];
    eprintln!("circle at the bottom: {bottom:?}");
    assert!(green(bottom[0]), "the top clears: {:?}", bottom[0]);
    assert!(
        red(bottom[1]),
        "the circle's top reaches the red half: {:?}",
        bottom[1]
    );
    assert!(
        blue(bottom[2]) && blue(bottom[3]),
        "the circle on the blue half: {bottom:?}"
    );

    // `center` to the right: an upright 9:16 picture gives the circle no room sideways, so it stays in the middle.
    // Run as stored, the stored picture's longer side is the shown one's height, and it would have gone to the bottom.
    let at = render(vec![circle(1.0, 0.5)]).await;
    let right = [at(320, 100), at(320, 300), at(320, 340)];
    eprintln!("circle asked to the right: {right:?}");
    assert!(red(right[0]), "the circle stays centred: {:?}", right[0]);
    assert!(
        green(right[1]) && green(right[2]),
        "below it clears: {right:?}"
    );

    // Memory: warm, a turned masked layer makes no GPU objects (the upright target, its uniform slot and the input
    // bind group are reused), wherever the circle goes.
    let made = gpu_objects_made();
    for y in [0.0, 0.5, 1.0] {
        let _ = render(vec![circle(0.5, y)]).await;
    }
    assert_eq!(
        gpu_objects_made(),
        made,
        "a warm upright frame made GPU objects"
    );

    // Run as stored, not upright (the test hook), the bottom setting leaves the circle in the middle
    // (y 78.75–281.25), red at (320, 100): what the checks above rule out.
    let mut once = async |p: Arc<Project>, format, size, upright: bool| {
        headless.set_upright(upright);
        let started = Instant::now();
        let f = headless.render(p, secs(0.5), format, size).await.unwrap();
        (f, started.elapsed().as_secs_f64() * 1000.0)
    };
    let bottom = Arc::new(phone_overlay(rect, vec![circle(0.5, 1.0)]));
    let (f, _) = once(bottom, FormatKind::Landscape, (640, 360), false).await;
    let as_stored = pixel(&f.bytes, f.stride, 320, 100);
    assert!(
        red(as_stored),
        "as stored the circle stays centred: {as_stored:?}"
    );

    // Cost of the upright pass: eight copies of the phone layer, each with one mask (to read the pass above the
    // noise), with the pass and without it (as stored), so the difference is eight passes. Two sizes: full-frame
    // 1080×1920 in a 1080×1920 frame, and a facecam drawn 303.75×540 in 1080p. Each size on its own (a frame of
    // another size costs the next render its readback buffers), with and without in turns that swap order (ABBA),
    // medians of 40; each render includes its readback.
    let one = |rect| {
        let mut p = phone_overlay(rect, vec![circle(0.5, 0.5)]);
        let phone = p.timeline.tracks[1].clone();
        for k in 1..8 {
            let mut copy = phone.clone();
            copy.id = format!("v{k}");
            copy.items[0].id = format!("phone{k}");
            p.timeline.tracks.push(copy);
        }
        Arc::new(p)
    };
    let sizes = [
        (
            one(bx(0.0, 0.0, 1.0, 1.0)),
            FormatKind::Portrait,
            (1080, 1920),
        ),
        (
            one(bx(0.25, 0.25, 0.5, 0.5)),
            FormatKind::Landscape,
            (1920, 1080),
        ),
    ];
    let mut medians = Vec::new();
    for (p, format, size) in &sizes {
        let mut ms: [Vec<f64>; 2] = Default::default();
        for upright in [true, false, true, false] {
            once(p.clone(), *format, *size, upright).await;
        }
        for k in 0..80 {
            let upright = (k % 4 == 0) || (k % 4 == 3);
            let t = once(p.clone(), *format, *size, upright).await.1;
            ms[usize::from(!upright)].push(t);
        }
        medians.push(ms.map(|mut v| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        }));
    }
    let [[full, full_stored], [cam, cam_stored]] = [medians[0], medians[1]];
    eprintln!(
        "upright pass, median ms/frame for eight masked phone layers: full-frame 1080×1920 {full:.2} vs \
         {full_stored:.2} as stored (+{:.3} a pass); facecam 304×540 in 1080p {cam:.2} vs {cam_stored:.2} \
         (+{:.3} a pass)",
        (full - full_stored) / 8.0,
        (cam - cam_stored) / 8.0,
    );
}

/// A frame with many moving layers with effects, each as big as an effect target goes (4 × 4 canvases, growing), stays
/// within the effect stage's budget for a frame: the layers past it are shown without their
/// effects, and the frame says so once, instead of asking for gigabytes of targets.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn many_moving_effect_layers_stay_within_the_frames_target_budget() {
    use std::sync::Arc;

    use klipolo_engine::headless::Headless;
    use klipolo_engine::render::effects::{
        MAX_FRAME_TARGET_BYTES, effect_target_peak, reset_effect_target_peak,
    };
    use klipolo_model::motion::{Key, Motion, MotionKeys};

    let _gpu = GPU.lock().await;
    // This frame's peak only (the GPU tests here run one at a time).
    reset_effect_target_peak();
    let key = |at: f64, v: f64| Key {
        t_us: secs(at),
        v,
        ease: None,
    };
    let grow = Motion {
        keys: Some(MotionKeys {
            scale: vec![key(0.0, 1.0), key(2.0, 2.0)],
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut p = project();
    p.timeline.main.clear();
    // 24 layers of about 38 MB of targets each (4096 × 2304, the most a target holds): 900 MB without the budget.
    p.timeline.tracks = (0..24)
        .map(|i| {
            let gradient =
                ItemContent::generator(builtin::GENERATOR_GRADIENT.into(), BTreeMap::new());
            let mut layer = item(
                &format!("g{i}"),
                0.0,
                2.0,
                bx(-1.5, -1.5, 4.0, 4.0),
                gradient,
            );
            layer.layout.motion = Some(grow.clone());
            layer.effects =
                vec![effect("v", builtin::EFFECT_VIGNETTE).with("amount", ParamValue::Number(0.5))];
            overlay_track(&format!("t{i}"), vec![layer])
        })
        .collect();
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();
    let f = headless
        .render(Arc::new(p), secs(1.0), Landscape, (1280, 720))
        .await
        .unwrap();
    let peak = effect_target_peak();
    println!("effect targets at most {} MB", peak >> 20);
    assert!(peak <= MAX_FRAME_TARGET_BYTES, "{} MB", peak >> 20);
    assert!(
        f.warnings
            .iter()
            .any(|w| w == klipolo_engine::api::FRAME_EFFECTS_LEFT_OUT),
        "{:?}",
        f.warnings
    );
}

/// A frame over the effect budget never shows a pixelated layer plain (protective effects fail closed). Fine black and
/// white stripes, pixelated on top of 24 layers as big as an effect target goes, show as pixelated cells (black/white
/// edges only at cell boundaries, 64 px apart) or not at all, never as the stripes (an edge every 20 px):
/// - under 24 layers through an identity vignette, which fill the budget for other effects, the pixelation still runs:
///   protective passes have their own, higher ceiling;
/// - under 24 pixelated layers, which use up that ceiling too, the layers past it (the stripes among them) are hidden,
///   and the frame says so.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_pixelated_layer_over_the_budget_is_never_shown_plain() {
    use std::sync::Arc;

    use klipolo_engine::headless::Headless;
    use klipolo_engine::render::effects::{
        MAX_PROTECTED_TARGET_BYTES, effect_target_peak, reset_effect_target_peak,
    };

    let _gpu = GPU.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("stripes.png");
    image::RgbaImage::from_fn(1024, 576, |x, _| {
        let v = if (x / 4) % 2 == 0 { 0 } else { 255 };
        image::Rgba([v, v, v, 255])
    })
    .save(&png)
    .unwrap();
    let pixelate = || effect("p", builtin::EFFECT_PIXELATE).with("size", ParamValue::Number(96.0));
    let frame_over = |under: EffectInstance| {
        let mut p = project();
        p.timeline.main.clear();
        let logo = p.assets.iter_mut().find(|a| a.id == "logo").unwrap();
        (logo.path, logo.width, logo.height) = (png.display().to_string(), 1024, 576);
        let solid = ItemContent::generator(
            builtin::GENERATOR_GRADIENT.into(),
            BTreeMap::from([
                ("startColor".into(), ParamValue::Text("#336699".into())),
                ("endColor".into(), ParamValue::Text("#336699".into())),
            ]),
        );
        let mut tracks: Vec<_> = (0..24)
            .map(|i| {
                let mut layer = item(
                    &format!("g{i}"),
                    0.0,
                    2.0,
                    bx(-1.5, -1.5, 4.0, 4.0),
                    solid.clone(),
                );
                layer.effects = vec![under.clone()];
                overlay_track(&format!("t{i}"), vec![layer])
            })
            .collect();
        // On top: the stripes, as big too, pixelated in 96 px cells.
        let mut secret = item(
            "secret",
            0.0,
            2.0,
            bx(-1.5, -1.5, 4.0, 4.0),
            ItemContent::Image {
                asset_id: "logo".into(),
            },
        );
        secret.effects = vec![pixelate()];
        tracks.push(overlay_track("s", vec![secret]));
        p.timeline.tracks = tracks;
        Arc::new(p)
    };
    // Where row 360 turns from black to white or back (greys between, from scaling, don't count).
    let edges = |f: &klipolo_engine::session::RenderedRgba| {
        let mut edges: Vec<u32> = Vec::new();
        let mut last = None;
        for x in 0..1280 {
            let dark = match pixel(&f.bytes, f.stride, x, 360)[0] {
                0..=63 => true,
                192..=255 => false,
                _ => continue,
            };
            if last.is_some_and(|d| d != dark) {
                edges.push(x);
            }
            last = Some(dark);
        }
        edges
    };
    let never_plain = |edges: &[u32]| {
        let closest = edges.windows(2).map(|w| w[1] - w[0]).min();
        assert!(
            closest.is_none_or(|gap| gap >= 48),
            "the stripes show plain: edges {edges:?}"
        );
    };
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();

    let vignette = effect("v", builtin::EFFECT_VIGNETTE).with("amount", ParamValue::Number(0.0));
    let f = headless
        .render(frame_over(vignette), secs(1.0), Landscape, (1280, 720))
        .await
        .unwrap();
    let found = edges(&f);
    println!("under vignettes: {} edges; {:?}", found.len(), f.warnings);
    never_plain(&found);
    assert!(
        !found.is_empty(),
        "pixelated, within the protective ceiling"
    );

    reset_effect_target_peak();
    let f = headless
        .render(frame_over(pixelate()), secs(1.0), Landscape, (1280, 720))
        .await
        .unwrap();
    let found = edges(&f);
    let peak = effect_target_peak();
    println!(
        "under pixelations: {} edges, targets at most {} MB; {:?}",
        found.len(),
        peak >> 20,
        f.warnings
    );
    never_plain(&found);
    assert!(peak <= MAX_PROTECTED_TARGET_BYTES, "{} MB", peak >> 20);
    assert!(
        f.warnings
            .iter()
            .any(|w| w == klipolo_engine::api::FRAME_PROTECTION_HIDDEN),
        "{:?}",
        f.warnings
    );
}

/// Text textures stay within their ceiling (an export's, `EXPORT_TEXT_BYTES`: `Headless` draws as
/// an export does): 50 text tracks, each a box of 4 × 4 canvases filled with letters, would hold about 1.2 GB of
/// textures at 1080p. Each is cut to the part that can show; past the
/// ceiling texts are laid out softer, and those that don't fit even at their softest aren't drawn this frame. The
/// letters still show and the frame says so. Each text is laid out at most about once, and a second, unchanged
/// frame lays nothing out (no layout every frame).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn fifty_huge_text_boxes_stay_within_the_text_ceiling() {
    use std::sync::Arc;

    use klipolo_engine::headless::Headless;
    use klipolo_engine::render::text::{
        EXPORT_TEXT_BYTES, TEXT_OVER_BUDGET, reset_text_texture_peak, text_layouts,
        text_texture_peak,
    };

    let _gpu = GPU.lock().await;
    let mut p = project();
    p.timeline.main.clear();
    p.timeline.tracks = huge_texts(50);
    reset_text_texture_peak();
    let p = Arc::new(p);
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();
    let before = text_layouts();
    let f = headless
        .render(p.clone(), secs(1.0), Landscape, (1920, 1080))
        .await
        .unwrap();
    let first = text_layouts() - before;
    headless
        .render(p, secs(1.0), Landscape, (1920, 1080))
        .await
        .unwrap();
    let second = text_layouts() - before - first;
    let peak = text_texture_peak();
    println!(
        "text textures at most {} MB; {first} layouts in the first frame, {second} in the second; {:?}",
        peak >> 20,
        f.warnings
    );
    assert!(first <= 2 * 50, "about one layout a text: {first}");
    assert_eq!(second, 0, "an unchanged frame lays nothing out");
    assert!(peak <= EXPORT_TEXT_BYTES, "{} MB", peak >> 20);
    assert!(
        f.warnings.iter().any(|w| w == TEXT_OVER_BUDGET),
        "{:?}",
        f.warnings
    );
    let lit = (0..1920)
        .step_by(4)
        .flat_map(|x| (400..680).step_by(4).map(move |y| (x, y)))
        .any(|(x, y)| pixel(&f.bytes, f.stride, x, y)[0] > 128);
    assert!(lit, "the letters still show across the middle of the frame");
}

/// `n` text tracks, each a box of 4 × 4 canvases centred on the frame and filled with 400 px letters (a texture is cut
/// to its ink): at 3840 × 2160 each is cut to about a 4096² texture (64 MiB), so four fill an export's
/// ceiling (`EXPORT_TEXT_BYTES`, 256 MiB), and at 1920 × 1080 to the frame and a margin round it (about 25 MB).
fn huge_texts(n: usize) -> Vec<klipolo_model::timeline::Track> {
    (0..n)
        .map(|i| {
            let letters = ItemContent::Text {
                text: "A".repeat(300),
                style: klipolo_model::timeline::TextStyle {
                    size: 400.0,
                    ..klipolo_model::timeline::TextStyle::default()
                },
            };
            let filled = item(
                &format!("t{i}"),
                0.0,
                2.0,
                bx(-1.5, -1.5, 4.0, 4.0),
                letters,
            );
            overlay_track(&format!("x{i}"), vec![filled])
        })
        .collect()
}

/// A text past a full ceiling is never silently missing: at 3840 × 2160 four huge texts fill
/// `EXPORT_TEXT_BYTES`, so a fifth has no room even near its softest. It is left out or drawn soft, and every frame
/// says so: an export stops on that note rather than finishing without it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_text_past_a_full_ceiling_at_4k_is_said_on_every_frame() {
    use std::sync::Arc;

    use klipolo_engine::headless::Headless;
    use klipolo_engine::render::text::{
        EXPORT_TEXT_BYTES, TEXT_OVER_BUDGET, reset_text_texture_peak, text_texture_peak,
    };

    let _gpu = GPU.lock().await;
    let mut p = project();
    p.timeline.main.clear();
    p.timeline.tracks = huge_texts(5);
    reset_text_texture_peak();
    let p = Arc::new(p);
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();
    for frame in 0..2 {
        let f = headless
            .render(p.clone(), secs(1.0), Landscape, (3840, 2160))
            .await
            .unwrap();
        println!("frame {frame}: {:?}", f.warnings);
        assert!(
            f.warnings.iter().any(|w| w == TEXT_OVER_BUDGET),
            "frame {frame} leaves a text out without saying so: {:?}",
            f.warnings
        );
    }
    let peak = text_texture_peak();
    assert!(peak <= EXPORT_TEXT_BYTES, "{} MB", peak >> 20);
}

/// A text whose lines overflow its box, past a full ceiling, isn't laid out again on every frame: its
/// box lies just below a 3840 × 2160 frame (its estimate needs nothing) but its lines reach well into it, and huge
/// texts have filled the ceiling. It is left out, with the note, and the next frame neither lays it out again nor
/// leaves it out unsaid.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_text_overflowing_its_box_past_the_ceiling_is_not_laid_out_every_frame() {
    use std::sync::Arc;

    use klipolo_engine::headless::Headless;
    use klipolo_engine::render::text::{TEXT_OVER_BUDGET, text_layouts};

    let _gpu = GPU.lock().await;
    let mut p = project();
    p.timeline.main.clear();
    let mut tracks = huge_texts(4);
    let long = item(
        "long",
        0.0,
        2.0,
        bx(-0.5, 1.05, 2.0, 0.05),
        text(&"overflowing words ".repeat(40)),
    );
    tracks.push(overlay_track("overflow", vec![long]));
    p.timeline.tracks = tracks;
    let p = Arc::new(p);
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();
    let (mut layouts, mut said) = (Vec::new(), Vec::new());
    for _ in 0..2 {
        let before = text_layouts();
        let f = headless
            .render(p.clone(), secs(1.0), Landscape, (3840, 2160))
            .await
            .unwrap();
        layouts.push(text_layouts() - before);
        said.push(f.warnings.iter().any(|w| w == TEXT_OVER_BUDGET));
    }
    println!("layouts per frame: {layouts:?}; said: {said:?}");
    assert!(layouts[0] >= 5, "every text is laid out once: {layouts:?}");
    assert_eq!(
        layouts[1], 0,
        "the next frame lays nothing out: {layouts:?}"
    );
    assert_eq!(
        said,
        [true, true],
        "the text left out is said on each frame"
    );
}

/// An export keeps the old text ceiling (`EXPORT_TEXT_BYTES`, 256 MiB). A 4K frame
/// of two huge filled boxes (about 64 MiB each) and a paragraph block exports sharp and says nothing; the preview's
/// ceiling (`MAX_TEXT_BYTES`, 96 MiB) softens the second huge box and says so.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_4k_export_keeps_the_old_text_ceiling_where_the_preview_softens() {
    use std::sync::Arc;

    use klipolo_engine::headless::Headless;
    use klipolo_engine::render::text::{
        EXPORT_TEXT_BYTES, MAX_TEXT_BYTES, TEXT_OVER_BUDGET, reset_text_texture_peak,
        text_texture_peak,
    };
    use klipolo_engine::session::{PreviewBackend, RenderRequest};

    let _gpu = GPU.lock().await;
    let mut p = project();
    p.timeline.main.clear();
    let mut tracks = huge_texts(2);
    let paragraph = ItemContent::Text {
        text: "A paragraph block of words that fills most of a slide. ".repeat(4),
        style: klipolo_model::timeline::TextStyle {
            size: 72.0,
            ..klipolo_model::timeline::TextStyle::default()
        },
    };
    let block = item("p", 0.0, 2.0, bx(0.05, 0.25, 0.9, 0.5), paragraph);
    tracks.push(overlay_track("p", vec![block]));
    p.timeline.tracks = tracks;
    let p = Arc::new(p);

    reset_text_texture_peak();
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();
    let f = headless
        .render(p.clone(), secs(1.0), Landscape, (3840, 2160))
        .await
        .unwrap();
    let peak = text_texture_peak();
    println!("export: text textures {} MB; {:?}", peak >> 20, f.warnings);
    assert!(
        !f.warnings.iter().any(|w| w == TEXT_OVER_BUDGET),
        "the export draws every text sharp: {:?}",
        f.warnings
    );
    assert!(
        peak > MAX_TEXT_BYTES && peak <= EXPORT_TEXT_BYTES,
        "{} MB",
        peak >> 20
    );

    let plan = frame_plan(
        &klipolo_engine::plan::PlanContext::new(p),
        secs(1.0),
        Landscape,
    );
    let mut preview = klipolo_engine::render::GpuBackend::new(
        klipolo_engine::render::mode::RenderMode::Contained,
    )
    .await
    .unwrap();
    let f = preview
        .render(RenderRequest {
            plan,
            width: 3840,
            height: 2160,
            output: klipolo_engine::present::Output::Rgba,
        })
        .await
        .unwrap()
        .finish()
        .await
        .unwrap();
    assert!(
        f.warnings.iter().any(|w| w == TEXT_OVER_BUDGET),
        "the preview softens a text and says so: {:?}",
        f.warnings
    );
}

/// A text far bigger than the frame and wholly off it keeps its layout while it waits to slide in: it
/// is laid out once, not on every frame.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_huge_text_wholly_off_the_frame_is_laid_out_once() {
    use std::sync::Arc;

    use klipolo_engine::headless::Headless;
    use klipolo_engine::render::text::text_layouts;

    let _gpu = GPU.lock().await;
    let mut p = project();
    p.timeline.main.clear();
    let off = item("off", 0.0, 2.0, bx(2.0, -1.5, 4.0, 4.0), text("A"));
    p.timeline.tracks = vec![overlay_track("x", vec![off])];
    let p = Arc::new(p);
    let mut headless = Headless::new(Registry::builtin()).await.unwrap();
    let mut layouts = Vec::new();
    for _ in 0..3 {
        let before = text_layouts();
        headless
            .render(p.clone(), secs(1.0), Landscape, (1920, 1080))
            .await
            .unwrap();
        layouts.push(text_layouts() - before);
    }
    assert_eq!(layouts, [1, 0, 0], "laid out once, then kept");
}

/// A person's title whose id is "captions" under the captions is laid out once, and an
/// out; the captions are their own layer (the caption renderer), never a text texture.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU"]
async fn a_title_named_captions_and_the_captions_each_lay_out_once() {
    use std::sync::Arc;

    use klipolo_engine::plan::{PlanContext, TextLayer};
    use klipolo_engine::present::Output;
    use klipolo_engine::render::GpuBackend;
    use klipolo_engine::render::text::text_layouts;
    use klipolo_engine::session::{PreviewBackend, RenderRequest};
    use klipolo_model::captions::{Captions, Pages};

    let _gpu = GPU.lock().await;
    let mut p = project();
    p.timeline.main.clear();
    p.captions.enabled = true;
    p.timeline.tracks = vec![overlay_track(
        "t",
        vec![item(
            "captions",
            0.0,
            3.0,
            bx(0.1, 0.1, 0.8, 0.2),
            text("A title"),
        )],
    )];
    let page = Pages::one_page(
        "v",
        secs(1.0),
        secs(2.0),
        &[("Hello world", secs(1.0), secs(1.8))],
    );
    let captions = Captions {
        style: klipolo_model::contrib::ContribId::builtin("caption.clean"),
        pages: BTreeMap::from([(Landscape, Arc::new(page))]),
    };
    let ctx = PlanContext::new(Arc::new(p)).with_captions(Arc::new(captions));
    let plan = frame_plan(&ctx, secs(1.5), Landscape);
    let texts: Vec<&TextLayer> = plan
        .layers
        .iter()
        .filter_map(|l| match l {
            Layer::Text(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts.iter().map(|t| (&*t.id, &*t.text)).collect::<Vec<_>>(),
        [("captions", "A title")],
        "the title is the one text"
    );
    assert!(matches!(plan.layers.last(), Some(Layer::Caption(_))));
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .unwrap()
        .wait_for_assets();
    let mut layouts = Vec::new();
    for _ in 0..3 {
        let before = text_layouts();
        let f = backend
            .render(RenderRequest {
                plan: plan.clone(),
                width: 1920,
                height: 1080,
                output: Output::Rgba,
            })
            .await
            .unwrap()
            .finish()
            .await
            .unwrap();
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
        layouts.push(text_layouts() - before);
    }
    assert_eq!(
        layouts,
        [1, 0, 0],
        "the title laid out once, then nothing per frame"
    );
}
