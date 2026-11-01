//! Scene targets' bounds on the GPU (lavapipe in CI): an offscreen node's target is only as big as its own box, so the
//! designs the scene presets are made of fit the budget's Normal class at 1080p (scenes have no Heavy); a normal-blend
//! node goes back as a quad, with no ping-pong partner; `SceneBytes` agrees with what a fresh pool really allocated for
//! the frame; a node moving through its motion keeps its target; and a turned glass card grabs its turned box.

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use klipolo_engine::render::effects::{
    effect_target_peak, gpu_objects_made, reset_effect_target_peak,
};
use klipolo_engine::render::scene::{SceneBytes, SceneSplit};
use klipolo_model::project::Project;
use support::*;

const HD: (u32, u32) = (1920, 1080);
const HD_LAYER: u64 = 1920 * 1080 * 4;
const PREVIEW: (u32, u32) = (1280, 720);
const PREVIEW_LAYER: u64 = 1280 * 720 * 4;

/// A shadowed line of text at `y`, in a box of the frame.
fn shadowed(id: &str, text: &str, y: f64, size: u32) -> String {
    format!(
        r##"{{"id":"{id}","type":"text","text":"{text}","box":[0.08,{y},0.5,0.08],"size":{size},"weight":700,
        "fill":"#FFFFFF","shadow":{{"x":4,"y":4,"blur":10,"color":"rgba(0, 0, 0, 0.6)"}}}}"##
    )
}

/// A scene of `nodes` (its `timeline`, if any), 2 s.
fn scene(nodes: &[String], timeline: &str) -> String {
    format!(
        r##"{{"scene":1,"duration":2,"nodes":[{}]{timeline}}}"##,
        nodes.join(",")
    )
}

/// What scene layer `p` holds on its first frame at `t` on a fresh device, `size` pixels, against what the pool
/// allocated for it: `SceneBytes` (its own target, at most `layer` bytes and only its box, its targets at their own
/// sizes beyond it and the partner, and its scratch) must be the pool's every byte (the pool's own count, not
/// `SceneBytes` itself).
async fn first_frame(
    (p, registry): (Project, Arc<klipolo_engine::registry::Registry>),
    t: f64,
    size: (u32, u32),
    layer: u64,
) -> SceneBytes {
    let mut h = Headless::new(registry).await.unwrap();
    reset_effect_target_peak();
    let (_, notes) = scene_frame(&mut h, &Arc::new(p), t, size).await;
    let bad: Vec<&String> = notes.iter().filter(|n| *n != "frame.fontMissing").collect();
    assert!(bad.is_empty(), "{notes:?}");
    let bytes = h.scene_stats().bytes;
    assert!(bytes.own > 0 && bytes.own <= layer, "{bytes:?}");
    assert_eq!(
        effect_target_peak(),
        bytes.own + bytes.partner + bytes.targets + bytes.scratch,
        "the pool against SceneBytes: {bytes:?}"
    );
    bytes
}

/// The budget's Normal: one extra layer target's worth and 8 MiB.
fn normal(s: SceneSplit) -> bool {
    s.extra_targets <= 1 && s.extra_bytes <= 8 << 20
}

/// At 1080p, a lone shadowed title, a masked group, a faded group around two shadowed titles and a glass lower third
/// are Normal; a plain title holds nothing extra (Light, at preview size). Each offscreen target is its node's box, not
/// a layer, and a faded group goes back as a quad (no partner), as a masked one does.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn common_designs_fit_normal_at_1080p() {
    let _gpu = gpu_turn().await;
    let run = |json: String, under: bool| async move {
        let dir = tempfile::tempdir().unwrap();
        let (mut p, registry) = scene_project(&json, 2.0);
        if under {
            stripes_under(&mut p, dir.path());
        }
        first_frame((p, registry), 1.0, HD, HD_LAYER).await
    };
    // Its shadow is baked into its atlas: nothing offscreen.
    let lone = run(scene(&[shadowed("t", "Ada Lovelace", 0.7, 64)], ""), false).await;
    assert_eq!(lone.targets, 0, "its shadow in its own pass: {lone:?}");
    assert_eq!(lone.partner, 0, "laid back as a quad: {lone:?}");
    assert!(
        normal(lone.split(HD_LAYER)),
        "a lone shadowed title is Normal: {lone:?}"
    );

    let masked = scene(
        &[
            r##"{"id":"spot","type":"ellipse","box":[0.1,0.6,0.3,0.3],"fill":"#FFFFFF","opacity":0}"##.into(),
            r##"{"type":"group","box":[0.05,0.6,0.5,0.3],"mask":{"node":"#spot"},"children":[
              {"type":"rect","box":[0,0,1,1],"fill":"#FF3B30"}]}"##
                .into(),
        ],
        "",
    );
    let masked = run(masked, false).await;
    assert!(
        masked.targets < HD_LAYER,
        "its group, its mask, the mask's pass: {masked:?}"
    );
    assert_eq!(
        masked.partner, 0,
        "masked, laid back as a quad: {masked:?}"
    );
    assert!(
        normal(masked.split(HD_LAYER)),
        "a masked group is Normal: {masked:?}"
    );

    let card = format!(
        r##"{{"id":"card","type":"group","box":[0,0,1,1],"opacity":0.7,"children":[
          {{"type":"rect","box":[0.05,0.62,0.6,0.3],"radius":12,"fill":"#1E2A3A"}},{},{}]}}"##,
        shadowed("name", "Ada Lovelace", 0.65, 64),
        shadowed("role", "Analyst", 0.78, 40)
    );
    let faded = run(scene(&[card], ""), false).await;
    assert_eq!(
        faded.partner, 0,
        "every node laid back as a quad: {faded:?}"
    );
    assert!(faded.targets < HD_LAYER, "{faded:?}");
    assert!(
        normal(faded.split(HD_LAYER)),
        "a faded group around two shadowed titles is Normal: {faded:?}"
    );

    let glass = r##"{"type":"backdrop","box":[0.05,0.62,0.6,0.3],"radius":24,"blur":16,"tint":"rgba(255, 255, 255, 0.2)"}"##;
    let glass = run(scene(&[glass.into()], ""), true).await;
    assert!(
        normal(glass.split(HD_LAYER)),
        "a glass lower third is Normal or Light: {glass:?}"
    );

    // The budget measures a class at preview size: there a plain title holds nothing past its own share (its stencil
    // and buffers within 4 MiB, even where a stencil is emulated at four bytes a pixel).
    let plain = r##"{"id":"t","type":"text","text":"Plain title","box":[0.08,0.7,0.5,0.08],"size":64,"fill":"#FFFFFF"}"##;
    let (p, registry) = scene_project(&scene(&[plain.into()], ""), 2.0);
    let light = first_frame((p, registry), 1.0, PREVIEW, PREVIEW_LAYER).await;
    let split = light.split(PREVIEW_LAYER);
    assert_eq!(
        (
            light.targets,
            light.partner,
            split.extra_targets,
            split.extra_bytes
        ),
        (0, 0, 0, 0),
        "Light: {light:?}"
    );
}

/// A lower third popping in with its card faded and its titles shadowed (a scale and a slide; the card offscreen, the
/// titles' shadows baked): after its first frames, a node's target and the scene's own keep their size through its
/// whole motion (the survey found their largest box), so no frame of it makes a target, a view, a stencil or a bind
/// group.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_popping_lower_third_makes_no_gpu_objects() {
    let _gpu = gpu_turn().await;
    let (p, registry) = scene_project(&popping(), 2.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    for t in [0.0, 0.02, 0.04] {
        scene_frame(&mut h, &p, t, G_SIZE).await;
    }
    let before = gpu_objects_made();
    let mut most = 0;
    for i in 0..30 {
        let (_, notes) = scene_frame(&mut h, &p, 0.05 + f64::from(i) * 0.05, G_SIZE).await;
        assert!(notes.iter().all(|n| n == "frame.fontMissing"), "{notes:?}");
        most = most.max(h.scene_stats().offscreen);
    }
    let made = gpu_objects_made() - before;
    // The titles' shadows are baked: the card alone goes offscreen.
    assert!(most >= 1, "the card offscreen: {most}");
    assert_eq!(made, 0, "30 frames of its motion made {made} GPU objects");
}

/// The lower third that pops in: a faded card (scaling from half its size) around two shadowed titles, one sliding in
/// and one turning.
fn popping() -> String {
    let card = r##"{"id":"card","type":"group","box":[0.05,0.55,0.6,0.35],"origin":"0% 100%","children":[
      {"type":"rect","box":[0,0,1,1],"radius":12,"fill":"#1E2A3A"},
      {"id":"name","type":"text","text":"Ada Lovelace","box":[0.05,0.08,0.9,0.45],"size":48,"weight":700,
       "fill":"#FFFFFF","shadow":{"x":4,"y":4,"blur":10,"color":"rgba(0, 0, 0, 0.6)"}},
      {"id":"role","type":"text","text":"Analyst","box":[0.05,0.58,0.6,0.32],"size":30,"fill":"#C8D2E0",
       "shadow":{"x":2,"y":2,"blur":6,"color":"rgba(0, 0, 0, 0.6)"}}]}"##
        .to_string();
    let timeline = r##","timeline":[
      {"from":"#card","at":0,"dur":0.6,"opacity":0,"scale":0.5,"ease":"back.out"},
      {"from":"#name","at":0.2,"dur":0.6,"x":"-120px","ease":"power3.out"},
      {"to":"#role","at":0.3,"dur":1.2,"rotate":6,"scale":1.3}]"##;
    scene(&[card], timeline)
}

/// The numbers for the report (run by hand; the gate skips `scene_frame_costs*`): the popping lower third's drawer CPU
/// on its first frame (its survey with it) and a frame after, and a frame's passes, at 1280 × 720 over 2 s. lavapipe
/// and a debug build: meaningful only against each other.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "run by hand: cargo test --test scene_bounds_gpu -- --ignored scene_frame_costs_of_a_lower_third"]
async fn scene_frame_costs_of_a_lower_third() {
    let _gpu = gpu_turn().await;
    let (p, registry) = scene_project(&popping(), 2.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let (mut cpu, mut passes, mut first, mut pixels) = (Vec::new(), Vec::new(), 0.0, 0);
    let mut survey = 0.0;
    for i in 0..60 {
        let drawn = klipolo_engine::render::effects::pass_pixels();
        scene_frame(&mut h, &p, 0.02 + f64::from(i) / 30.0, (1280, 720)).await;
        let stats = h.scene_stats();
        if i == 0 {
            (first, survey) = (stats.cpu_us_last, stats.survey_us);
        } else if i >= 6 {
            cpu.push(stats.cpu_us_last);
            passes.push(h.passes_last_frame());
            pixels = pixels.max(klipolo_engine::render::effects::pass_pixels() - drawn);
        }
    }
    cpu.sort_by(f64::total_cmp);
    passes.sort_unstable();
    println!(
        "lower third: drawer CPU first frame {first:.0} µs (its survey {survey:.0} µs), then median {:.0} µs (p90 {:.0}); \
         passes a frame {}–{}; effect passes at most {:.2} MP a frame; {:?}",
        cpu[cpu.len() / 2],
        cpu[cpu.len() * 9 / 10],
        passes[0],
        passes[passes.len() - 1],
        pixels as f64 / 1e6,
        h.scene_stats().bytes
    );
}

/// A glass card held turned 3° grabs, and blurs, its turned box, not the square its diagonal makes; one that spins
/// keeps the square on every frame, quarter turns among them, so it makes no GPU object as it turns.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_turned_glass_card_grabs_its_turned_box() {
    let _gpu = gpu_turn().await;
    let card = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"backdrop","box":[0,0,1,1],"radius":12,"blur":12,"tint":"rgba(255, 255, 255, 0.2)"}]}"##;
    let dir = tempfile::tempdir().unwrap();
    let (mut p, registry) = project_with_scene_in("g", card, 0.0, 2.0, bx(0.05, 0.4, 0.9, 0.2));
    scene_item(&mut p, "g").layout.rotation = 3.0;
    stripes_under(&mut p, dir.path());
    let held = first_frame((p.clone(), registry.clone()), 1.0, G_SIZE, 576 * 72 * 4).await;
    // The grab is the turned box, 579 × 103 (its copy blurred at half that); the diagonal's square would be 581 × 360.
    let (turned, square) = (579 * 103 * 4, 581 * 360 * 4);
    assert!(
        held.targets >= turned && held.targets < square,
        "the turned box, not the square: {held:?}"
    );
    let mut spin = p;
    scene_item(&mut spin, "g").layout.rotation = 0.0;
    let layer = scene_item(&mut spin, "g").clone();
    let mut layer = animated(layer, None, None);
    layer.layout.motion = Some(klipolo_model::motion::Motion {
        looping: Some(preset("spin")),
        ..Default::default()
    });
    *scene_item(&mut spin, "g") = layer;
    let spin = Arc::new(spin);
    let mut h = Headless::new(registry).await.unwrap();
    for t in [0.01, 0.02, 0.03] {
        scene_frame(&mut h, &spin, t, G_SIZE).await;
    }
    let before = gpu_objects_made();
    // 0.3 s a turn: 0.075 s is a quarter, 0.15 s a half.
    for i in 0..24 {
        scene_frame(&mut h, &spin, 0.3 + f64::from(i) * 0.0125, G_SIZE).await;
    }
    let made = gpu_objects_made() - before;
    assert_eq!(
        made, 0,
        "a spin through its quarter turns made {made} GPU objects"
    );
}
