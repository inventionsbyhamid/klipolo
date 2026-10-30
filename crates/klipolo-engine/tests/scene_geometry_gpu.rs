//! The scene geometry, in pixels (lavapipe in CI): goldens made
//! before offscreen nodes drew into targets of their own box, and compared on every later commit within 2/255. A faded
//! card around two shadowed titles (quads laid back into a box-sized parent); the lower third that pops in, mid-motion
//! (targets whose top left follows a moving box); and a multiply group and a tilted group inside a faded group (the host
//! pass into a box-sized parent). Two more were made earlier (the scene drew into the
//! layer's whole target, every text shadow through the offscreen chain): the lower third that pops in with no panel
//! behind it (the scene's own target is only its card's box), and a shadowed title fading in (its shadow baked, faded
//! with it).

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use support::*;

/// A shadowed line of text at `y`, in a box of the frame.
fn shadowed(id: &str, text: &str, y: f64, size: u32) -> String {
    format!(
        r##"{{"id":"{id}","type":"text","text":"{text}","box":[0.08,{y},0.5,0.08],"size":{size},"weight":700,
        "fill":"#FFFFFF","shadow":{{"x":4,"y":4,"blur":10,"color":"rgba(0, 0, 0, 0.6)"}}}}"##
    )
}

/// A scene of `nodes` (its `timeline`, if any), 2 s, over a light panel.
fn scene(nodes: &[String], timeline: &str) -> String {
    let panel = r##"{"type":"rect","box":[0,0,1,1],"fill":"#C9D2DE"}"##.to_string();
    bare(&[&[panel], nodes].concat(), timeline)
}

/// A scene of `nodes` (its `timeline`, if any), 2 s, with nothing behind them.
fn bare(nodes: &[String], timeline: &str) -> String {
    format!(
        r##"{{"scene":1,"duration":2,"nodes":[{}]{timeline}}}"##,
        nodes.join(",")
    )
}

/// The lower third that pops in (`scene_bounds_gpu`'s `popping`): a faded card scaling up around a shadowed title
/// sliding in and one turning, and its timeline.
fn popping() -> (String, &'static str) {
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
    (card, timeline)
}

/// Scene `json` at `t`, against golden `name`.
async fn golden(json: &str, t: f64, name: &str) {
    let (p, registry) = scene_project(json, 2.0);
    let mut h = Headless::new(registry).await.unwrap();
    let (img, notes) = scene_frame(&mut h, &Arc::new(p), t, G_SIZE).await;
    let bad: Vec<&String> = notes.iter().filter(|n| *n != "frame.fontMissing").collect();
    assert!(bad.is_empty(), "{notes:?}");
    scene_golden(name, &img);
}

/// The `faded` design: a card at 70 % around two shadowed titles, every node laid back as a quad.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_faded_card_around_shadowed_titles() {
    let _gpu = gpu_turn().await;
    let card = format!(
        r##"{{"id":"card","type":"group","box":[0,0,1,1],"opacity":0.7,"children":[
          {{"type":"rect","box":[0.05,0.62,0.6,0.3],"radius":12,"fill":"#1E2A3A"}},{},{}]}}"##,
        shadowed("name", "Ada Lovelace", 0.65, 64),
        shadowed("role", "Analyst", 0.78, 40)
    );
    golden(&scene(&[card], ""), 1.0, "geometry-faded").await;
}

/// The lower third that pops in, mid-motion.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_popping_lower_third_mid_motion() {
    let _gpu = gpu_turn().await;
    let (card, timeline) = popping();
    golden(&scene(&[card], timeline), 0.35, "geometry-popping-0.35").await;
}

/// The lower third that pops in with nothing behind it, mid-motion: the scene draws into a target of
/// it drew into the layer's whole.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_popping_lower_third_on_its_own() {
    let _gpu = gpu_turn().await;
    let (card, timeline) = popping();
    golden(&bare(&[card], timeline), 0.35, "geometry-popping-bare-0.35").await;
}

/// A title with an amber shadow (so it shows over nothing) fading in on its own, half way: its baked
/// it, as the offscreen chain's did.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_shadowed_title_fading_in() {
    let _gpu = gpu_turn().await;
    let title = r##"{"id":"name","type":"text","text":"Ada Lovelace","box":[0.08,0.65,0.5,0.08],"size":64,"weight":700,
      "fill":"#FFFFFF","shadow":{"x":6,"y":6,"blur":12,"color":"rgba(255, 196, 0, 0.9)"}}"##
        .to_string();
    let timeline = r##","timeline":[{"from":"#name","at":0,"dur":1,"opacity":0}]"##;
    golden(&bare(&[title], timeline), 0.5, "geometry-shadow-fading-0.5").await;
}

/// The same title with its shadow blurred 40 px, which the text pass bakes small at step 2 (σ
/// about 6.7 atlas texels at this size), against the frame an earlier version drew through the offscreen chain
/// (`geometry-shadow-wide-0.5`, made there), within the small bake's 5/255 across the cell and the
/// chain's own rounding.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_wide_shadowed_title_fading_in() {
    let _gpu = gpu_turn().await;
    let title = r##"{"id":"name","type":"text","text":"Ada Lovelace","box":[0.08,0.65,0.5,0.08],"size":64,"weight":700,
      "fill":"#FFFFFF","shadow":{"x":6,"y":6,"blur":40,"color":"rgba(255, 196, 0, 0.9)"}}"##
        .to_string();
    let timeline = r##","timeline":[{"from":"#name","at":0,"dur":1,"opacity":0}]"##;
    let (p, registry) = scene_project(&bare(&[title], timeline), 2.0);
    let mut h = Headless::new(registry).await.unwrap();
    let (img, notes) = scene_frame(&mut h, &Arc::new(p), 0.5, G_SIZE).await;
    assert!(notes.iter().all(|n| n == "frame.fontMissing"), "{notes:?}");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/scene/geometry-shadow-wide-0.5.png");
    let want = image::open(path).unwrap().to_rgba8();
    let (within, mean) = klipolo_engine::test_support::compare(&img, &want, 6);
    let (exact, _) = klipolo_engine::test_support::compare(&img, &want, 2);
    println!(
        "wide shadow: {:.3} % within 6/255, {:.3} % within 2/255, mean {mean:.4}",
        within * 100.0,
        exact * 100.0
    );
    assert!(
        within >= 0.999 && mean < 0.5,
        "{:.3} % within 6/255 of the offscreen chain's frame, mean {mean:.4}",
        within * 100.0
    );
}

/// A multiply group and a group turned in depth, both inside a group faded to 80 %: each laid back by the host pass
/// into its parent's box-sized target.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn host_lays_inside_a_faded_group() {
    let _gpu = gpu_turn().await;
    let group = r##"{"type":"group","box":[0.05,0.1,0.9,0.8],"opacity":0.8,"children":[
      {"type":"rect","box":[0,0.45,1,0.5],"radius":16,"fill":"#2D6CDF"},
      {"type":"group","box":[0.05,0.3,0.4,0.6],"blend":"multiply","children":[
        {"type":"ellipse","box":[0,0,1,1],"fill":"#FFD200"}]},
      {"type":"group","box":[0.55,0.3,0.4,0.6],"rotateY":35,"children":[
        {"type":"rect","box":[0,0,1,1],"radius":10,"fill":"#2BB673"},
        {"type":"star","box":[0.25,0.2,0.5,0.6],"sides":5,"fill":"#FFFFFF"}]}]}"##
        .to_string();
    golden(&scene(&[group], ""), 1.0, "geometry-host-in-faded").await;
}
