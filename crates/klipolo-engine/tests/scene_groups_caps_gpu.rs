//! Scene groups' caps and costs on the GPU (lavapipe in CI): at most four groups offscreen at once (a fifth draws in
//! place and is said once), untrusted scenes held to every cap, and a warm frame of every group feature making no GPU
//! object (what a scene holds is `scene_memory_gpu.rs`).

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use support::*;

/// Five faded groups side by side (an unchecked scene: the check refuses a fifth).
fn faded(groups: usize) -> String {
    let g: Vec<String> = (0..groups)
        .map(|i| {
            format!(
                r##"{{"type":"group","box":[{x},0.3,0.15,0.4],"opacity":0.5,"children":[
              {{"type":"rect","box":[0,0,1,1],"fill":"#FFFFFF"}},{{"type":"rect","box":[0.2,0.2,1,1],"fill":"#FFFFFF"}}]}}"##,
                x = 0.02 + 0.19 * i as f64
            )
        })
        .collect();
    format!(r##"{{"scene":1,"duration":2,"nodes":[{}]}}"##, g.join(","))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_fifth_offscreen_group_is_drawn_plain_and_said_once() {
    let _gpu = gpu_turn().await;
    let passes = |json: String| async move {
        let registry = unchecked_scene_registry(&[("g", &json)]);
        let p = project_of("g", 2.0);
        let p = Arc::new(p);
        let mut h = Headless::new(registry).await.unwrap();
        let mut notes = Vec::new();
        let mut last = (0, 0, 0);
        for t in [0.5, 1.0, 1.5] {
            let (_, n) = scene_frame(&mut h, &p, t, G_SIZE).await;
            notes.extend(n);
            last = (h.passes_last_frame(), h.scene_stats().offscreen, 0);
        }
        (notes, last)
    };
    let (four, (four_passes, four_off, _)) = passes(faded(4)).await;
    assert!(four.is_empty(), "{four:?}");
    assert_eq!(four_off, 4);
    let (five, (five_passes, five_off, _)) = passes(faded(5)).await;
    let said = five
        .iter()
        .filter(|n| *n == "frame.sceneTooManyGroups")
        .count();
    assert_eq!(said, 1, "said once: {five:?}");
    assert_eq!(five_off, 4, "four offscreen, the fifth in place");
    assert_eq!(five_passes, four_passes, "the fifth adds no pass");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn untrusted_scenes_are_held_to_the_caps() {
    let _gpu = gpu_turn().await;
    // Unchecked: eleven faded groups nested seven deep and side by side, forty emitters of fifty, ten backdrops and a
    // dozen generator fills, every one past what Klipolo draws in a frame.
    let nested = (0..7).fold(r##"{"type":"rect","box":[0,0,1,1],"fill":"#FFFFFF"}"##.to_string(), |c, _| {
        format!(r##"{{"type":"group","box":[0.05,0.05,0.9,0.9],"opacity":0.9,"blur":2,"children":[{c}]}}"##)
    });
    let mut nodes = vec![nested];
    for i in 0..4 {
        nodes.push(format!(
            r##"{{"type":"group","box":[{x},0.8,0.1,0.1],"opacity":0.5,"children":[{{"type":"rect","box":[0,0,1,1],"fill":"#FF0000"}}]}}"##,
            x = 0.1 * f64::from(i)
        ));
    }
    for _ in 0..40 {
        nodes.push(
            r##"{"type":"emitter","box":[0.5,0.5,0.01,0.01],"burst":50,"life":[2,2],"child":{"type":"ellipse","box":[0,0,"8px","8px"],"fill":"#00FF00"}}"##
                .into(),
        );
    }
    for _ in 0..10 {
        nodes.push(
            r##"{"type":"backdrop","box":[0.1,0.1,0.2,0.2],"blur":8,"tint":"rgba(0,0,255,0.3)"}"##
                .into(),
        );
    }
    for _ in 0..12 {
        nodes.push(
            r##"{"type":"rect","box":[0.6,0.1,0.3,0.3],"fill":{"generator":"klipolo.builtin/generator.gradient"}}"##
                .into(),
        );
    }
    let json = format!(
        r##"{{"scene":1,"duration":2,"nodes":[{}]}}"##,
        nodes.join(",")
    );
    let registry = unchecked_scene_registry(&[("g", &json)]);
    let p = project_of("g", 2.0);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let started = std::time::Instant::now();
    let mut notes = Vec::new();
    for t in [0.2, 0.6, 1.0] {
        let (_, n) = scene_frame(&mut h, &p, t, G_SIZE).await;
        notes.extend(n);
        let stats = h.scene_stats();
        assert!(
            stats.offscreen <= 8,
            "at most eight offscreen: {}",
            stats.offscreen
        );
        assert!(
            stats.bytes.targets <= 24 * 640 * 360 * 4,
            "bounded targets: {:?}",
            stats.bytes
        );
    }
    assert!(
        started.elapsed().as_secs() < 60,
        "drawn in {:?}",
        started.elapsed()
    );
    for code in ["frame.sceneTooManyGroups", "frame.sceneReduced"] {
        assert_eq!(
            notes.iter().filter(|n| *n == code).count(),
            1,
            "{code} once: {notes:?}"
        );
    }
}

/// Every group feature, moving: a faded group with a masked one inside, a tilting card filled by a generator, a
/// blurred star, a titled shadow, a glass backdrop over stripes, emoji and shape particles.
const BUSY: &str = r##"{"scene":1,"duration":4,"nodes":[
  {"type":"ellipse","id":"spot","box":[0.1,0.1,0.3,0.3],"fill":"#FFFFFF","opacity":0},
  {"type":"group","id":"fade","box":[0,0,1,1],"opacity":0.6,"children":[
    {"type":"group","box":[0,0,1,1],"mask":{"node":"#spot","mode":"luma"},"children":[
      {"type":"rect","box":[0.05,0.05,0.4,0.4],"fill":"#FF3B30"}]},
    {"type":"group","id":"tilt","box":[0.5,0.1,0.4,0.4],"rotateY":30,"children":[
      {"type":"rect","box":[0,0,1,1],"fill":{"generator":"klipolo.builtin/generator.gradient"}}]}]},
  {"type":"star","id":"star","box":[0.1,0.55,0.2,0.3],"blur":4,"fill":"#FFD200"},
  {"type":"text","text":"Busy","box":[0.05,0.4,0.4,0.15],"size":60,"fill":"#FFFFFF",
   "shadow":{"x":4,"y":4,"blur":6,"color":"rgba(0,0,0,0.6)"}},
  {"type":"backdrop","box":[0.4,0.55,0.25,0.3],"radius":16,"blur":10,"tint":"rgba(255,255,255,0.2)"},
  {"type":"emitter","box":[0.5,0.5,0.05,0.05],"burst":40,"rate":20,"life":[1,2],"speed":[100,300],"gravity":300,
   "child":{"type":"star","box":[0,0,"12px","12px"],"fill":"#00FFAA"}},
  {"type":"emitter","box":[0.8,0.5,0.05,0.05],"burst":10,"life":[3,3],"speed":[50,100],
   "child":{"type":"image","emoji":"✨","box":[0,0,"40px","40px"]}}],
  "timeline":[{"to":"#tilt","at":0,"dur":4,"rotateY":-30},{"to":"#fade","at":0,"dur":4,"opacity":0.9},
    {"to":"#star","at":0,"dur":4,"blur":12,"rotate":90}]}"##;

/// Performance first: once warm, frames of all of it make no GPU object (targets, bind groups, uniform slots and
/// pipelines are the pool's and the caches' own) and hold no frame.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_warm_frame_of_everything_makes_no_gpu_objects() {
    let _gpu = gpu_turn().await;
    let dir = tempfile::tempdir().unwrap();
    let (mut p, registry) = scene_project(BUSY, 4.0);
    stripes_under(&mut p, dir.path());
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    for i in 0..12 {
        scene_frame(&mut h, &p, 0.5 + f64::from(i) / 30.0, G_SIZE).await;
    }
    let before = klipolo_engine::render::effects::gpu_objects_made();
    for i in 0..30 {
        let (_, notes) = scene_frame(&mut h, &p, 1.0 + f64::from(i) / 30.0, G_SIZE).await;
        let bad: Vec<&String> = notes.iter().filter(|n| *n != "frame.fontMissing").collect();
        assert!(bad.is_empty(), "{notes:?}");
    }
    let made = klipolo_engine::render::effects::gpu_objects_made() - before;
    assert_eq!(made, 0, "30 warm frames made {made} GPU objects");
    let stats = h.scene_stats();
    // The faded, masked and tilted groups, the blurred star (the titled shadow is baked).
    assert!(stats.offscreen >= 4, "drawn offscreen: {}", stats.offscreen);
    assert_eq!(stats.held_frames, 0, "a backdrop is never held");
}

/// The numbers for the report (run by hand; the gate skips `scene_frame_costs*`): the busy scene's drawer CPU a frame
/// (and its first), and the frame's passes and the pixels its effect passes drew, over stripes, at 1280 × 720.
/// lavapipe and a debug build: meaningful only against each other.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "run by hand: cargo test --test scene_groups_caps_gpu -- --ignored scene_frame_costs_of_groups"]
async fn scene_frame_costs_of_groups() {
    let _gpu = gpu_turn().await;
    let dir = tempfile::tempdir().unwrap();
    let (mut p, registry) = scene_project(BUSY, 4.0);
    stripes_under(&mut p, dir.path());
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let (mut cpu, mut whole, mut first, mut pixels) = (Vec::new(), Vec::new(), 0.0, 0);
    let mut survey = 0.0;
    for i in 0..72 {
        let started = std::time::Instant::now();
        let drawn = klipolo_engine::render::effects::pass_pixels();
        scene_frame(&mut h, &p, 0.5 + f64::from(i) / 30.0, (1280, 720)).await;
        if i == 0 {
            (first, survey) = (h.scene_stats().cpu_us_last, h.scene_stats().survey_us);
        }
        if i >= 12 {
            whole.push(started.elapsed().as_secs_f64() * 1000.0);
            cpu.push(h.scene_stats().cpu_us_last);
            pixels = pixels.max(klipolo_engine::render::effects::pass_pixels() - drawn);
        }
    }
    cpu.sort_by(f64::total_cmp);
    whole.sort_by(f64::total_cmp);
    let stats = h.scene_stats();
    println!(
        "busy scene: drawer CPU first frame {first:.0} µs (its survey {survey:.0} µs), then median {:.0} µs (p90 {:.0}); \
         whole frame median {:.1} ms; \
         {} passes; {} offscreen; effect passes at most {:.2} MP a frame; {:?}",
        cpu[cpu.len() / 2],
        cpu[cpu.len() * 9 / 10],
        whole[whole.len() / 2],
        h.passes_last_frame(),
        stats.offscreen,
        pixels as f64 / 1e6,
        stats.bytes
    );
}
