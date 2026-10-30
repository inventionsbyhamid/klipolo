//! What a scene holds on the GPU: `SceneBytes` counts the
//! targets its passes held of the pool beyond its own and the lay's ping-pong partner (the backdrop's grab and copies
//! among them), each at its own size, and its letters' scratch and its fills at theirs, so its split lands in the
//! classes (`scene_bounds_gpu.rs` checks it against the pool);
//! and a glass card under its layer's motion keeps one grab size, so it makes no GPU object once warm.

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use klipolo_engine::render::effects::gpu_objects_made;
use klipolo_engine::render::scene::{SCENE_OWN_BYTES, reset_scene_bytes_peak, scene_bytes_peak};
use support::*;

/// A 640 × 360 layer's target.
const LAYER: u64 = 640 * 360 * 4;

/// `n` groups side by side, each faded or masked by a hidden circle of its own, and a title.
fn siblings(n: usize, masked: bool) -> String {
    let mut nodes = vec![
        r##"{"type":"text","text":"Counted","box":[0.1,0.05,0.8,0.2],"size":60,"fill":"#FFFFFF"}"##
            .to_string(),
    ];
    for i in 0..n {
        let x = 0.02 + 0.19 * i as f64;
        let look = if masked {
            nodes.push(format!(
                r##"{{"type":"ellipse","id":"m{i}","box":[{x},0.3,0.15,0.4],"fill":"#FFFFFF","opacity":0}}"##
            ));
            format!(r##""mask":{{"node":"#m{i}"}}"##)
        } else {
            r#""opacity":0.5"#.to_string()
        };
        nodes.push(format!(
            r##"{{"type":"group","box":[{x},0.3,0.15,0.4],{look},"children":[
              {{"type":"rect","box":[0,0,1,1],"fill":"#FFFFFF"}}]}}"##
        ));
    }
    format!(
        r##"{{"scene":1,"duration":2,"nodes":[{}]}}"##,
        nodes.join(",")
    )
}

/// What scene `json` (over stripes when `under`) holds at `t`.
async fn held(json: &str, t: f64, under: bool) -> klipolo_engine::render::scene::SceneStats {
    let dir = tempfile::tempdir().unwrap();
    let (mut p, registry) = scene_project(json, 2.0);
    if under {
        stripes_under(&mut p, dir.path());
    }
    let mut h = Headless::new(registry).await.unwrap();
    let (_, notes) = scene_frame(&mut h, &Arc::new(p), t, G_SIZE).await;
    assert!(notes.is_empty(), "{notes:?}");
    h.scene_stats()
}

/// Three faded sibling groups hold one target at a time (the pool hands the next the
/// one the last laid back), and only one of their own box's size, laid back as quads (no partner); two
/// masked ones hold their box's group, mask and masked targets (m9: each mask drawn just before its lay-back), laid
/// back as quads too. Its text is the scene's too, and the peak follows all of it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn scene_memory_is_counted() {
    let _gpu = gpu_turn().await;
    reset_scene_bytes_peak();
    let stats = held(&siblings(3, false), 1.0, false).await;
    let bytes = stats.bytes;
    assert_eq!(stats.offscreen, 3);
    assert_eq!((bytes.partner, bytes.scratch), (0, 0), "{bytes:?}");
    assert!(
        bytes.targets > 0 && bytes.targets <= LAYER / 8,
        "one group's box at a time, not three: {bytes:?}"
    );
    assert!(
        bytes.atlas > 0 && bytes.atlas == stats.text_bytes,
        "its text: {bytes:?} {stats:?}"
    );
    let peak = scene_bytes_peak();
    let want = bytes.total();
    assert!(
        (peak as f64 - want as f64).abs() <= 0.05 * want as f64,
        "peak {peak} against {want} ({bytes:?})"
    );
    let split = bytes.split(LAYER);
    let small = bytes.buffers + bytes.atlas;
    assert_eq!(split.extra_targets, 1);
    assert_eq!(split.extra_bytes, small.saturating_sub(SCENE_OWN_BYTES));
    assert_eq!(split.own, bytes.own + small.min(SCENE_OWN_BYTES));
    assert!(
        bytes.own < LAYER,
        "its own target is what it draws: {bytes:?}"
    );
    let masked = held(&siblings(2, true), 1.0, false).await.bytes;
    assert_eq!(
        masked.partner, 0,
        "a masked group is laid back as a quad: {masked:?}"
    );
    assert!(
        masked.targets > 0 && masked.targets <= LAYER / 2,
        "a group, its mask and the masked group, at their box: {masked:?}"
    );
}

/// The classes from the split. A blur-in title holds no extra target, only its letters' scratch at its row
/// class (two targets, as the blur is drawn small: 2 MiB): Normal. A glass card holds one, its blurred copy (the grab goes with the blur that reads it last, and
/// the scene's own target takes its place). A
/// generator fill is extra bytes at its own size, never a layer target.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_scenes_split_lands_in_its_budget_class() {
    let _gpu = gpu_turn().await;
    let title = r##"{"scene":1,"duration":2,"nodes":[
      {"id":"t","type":"text","text":"BLURRED","split":"chars","box":[0.05,0.3,0.9,0.4],"size":140,"weight":800,
       "fill":"#14171C","align":"center"}],
      "timeline":[{"from":"#t","at":0,"dur":0.5,"blur":16,"stagger":0.08}]}"##;
    let bytes = held(title, 0.2, false).await.bytes;
    let split = bytes.split(LAYER);
    assert_eq!((bytes.targets, bytes.partner), (0, 0), "{bytes:?}");
    assert_eq!(
        bytes.scratch,
        2 * 2048 * 128 * 4,
        "its scratch's row class, twice"
    );
    assert_eq!(split.extra_targets, 0);
    assert!(split.extra_bytes <= 8 << 20, "Normal: {split:?}");
    let glass = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"backdrop","box":[0.25,0.25,0.5,0.5],"radius":24,"blur":24,"tint":"rgba(255, 255, 255, 0.25)"}]}"##;
    let bytes = held(glass, 1.0, true).await.bytes;
    // Its blur halves (a 48-pixel blur): the copy and its blur's second pass at a quarter of the layer each; the grab is
    // the scene's own target after (the pool's count).
    assert_eq!(bytes.targets, LAYER / 2, "one blurred copy: {bytes:?}");
    assert_eq!(bytes.split(LAYER).extra_targets, 1);
    let filled = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"rect","box":[0.6,0.1,0.3,0.3],"fill":{"generator":"klipolo.builtin/generator.gradient"}}]}"##;
    let bytes = held(filled, 1.0, false).await.bytes;
    let split = bytes.split(LAYER);
    assert!(
        bytes.scratch > 0 && bytes.scratch < LAYER,
        "its fill at its own size: {bytes:?}"
    );
    assert_eq!((bytes.targets, split.extra_targets), (0, 0));
    assert!(split.extra_bytes >= bytes.scratch);
}

/// A glass lower third: a card filling its layer.
const CARD: &str = r##"{"scene":1,"duration":4,"nodes":[
  {"type":"backdrop","box":[0.05,0.1,0.9,0.8],"radius":24,"blur":16,"tint":"rgba(255, 255, 255, 0.2)"}]}"##;

/// Through a `pop` (its size changing every frame) and a `slide` up from the frame's edge (its box cut by the
/// frame less and less), a glass card's grab keeps the size of its layer's largest box, inside the frame, so after its
/// first frames the card makes no target, view or bind group. It grabbed the moving box before: new targets every
/// frame.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_moving_glass_card_makes_no_gpu_objects() {
    let _gpu = gpu_turn().await;
    for enter in ["pop", "slide"] {
        let dir = tempfile::tempdir().unwrap();
        let (mut p, registry) =
            project_with_scene_in("g", CARD, 0.0, 4.0, bx(0.05, 0.6, 0.5, 0.35));
        let layer = scene_item(&mut p, "g").clone();
        *scene_item(&mut p, "g") = animated(layer, Some(enter), None);
        stripes_under(&mut p, dir.path());
        let p = Arc::new(p);
        let mut h = Headless::new(registry).await.unwrap();
        for t in [0.02, 0.04, 0.06] {
            scene_frame(&mut h, &p, t, G_SIZE).await;
        }
        let before = gpu_objects_made();
        for i in 0..30 {
            let (_, notes) = scene_frame(&mut h, &p, 0.07 + f64::from(i) * 0.008, G_SIZE).await;
            assert!(notes.is_empty(), "{enter}: {notes:?}");
        }
        let made = gpu_objects_made() - before;
        assert_eq!(
            made, 0,
            "{enter}: 30 frames of its motion made {made} GPU objects"
        );
    }
}
