//! A scene's own target on the GPU (lavapipe in CI): a lower third in a full-frame layer draws into a target only as
//! big as its own box, so three of them at 4K hold a small share of the effect pool, and all three are held while they
//! hold still; a scene's passes have a stencil only when they draw a translucent stroke; a shadowed title and role draw
//! their shadows in the text pass, with no offscreen pass.

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use klipolo_engine::registry::Registry;
use klipolo_engine::render::effects::{
    effect_target_peak, gpu_objects_made, reset_effect_target_peak,
};
use klipolo_model::project::Project;
use klipolo_model::timeline::ItemContent;
use support::*;

const UHD: (u32, u32) = (3840, 2160);
const UHD_LAYER: u64 = 3840 * 2160 * 4;
const HD: (u32, u32) = (1920, 1080);

/// `n` full-frame layers showing `json` from 0 s for 4 s, each on its own track.
fn layers(json: &str, n: usize) -> (Project, Arc<Registry>) {
    let registry = scene_registry(&[("lt", json)]);
    let mut p = Project::new("Lower thirds");
    p.id = "lower-thirds".into();
    for k in 0..n {
        let content = ItemContent::Generator {
            source: scene_id("lt").as_str().into(),
            params: Default::default(),
            still: false,
            offset_us: 0,
            span_us: None,
            seed_from: None,
        };
        let layer = item(&format!("lt{k}"), 0.0, 4.0, bx(0.0, 0.0, 1.0, 1.0), content);
        p.timeline
            .tracks
            .push(overlay_track(&format!("t{k}"), vec![layer]));
    }
    (p, registry)
}

/// The lower third with a shadow under its title and its role.
fn shadowed_lower_third() -> String {
    let shadow = r#","shadow":{"x":3,"y":3,"blur":10,"color":"rgba(0, 0, 0, 0.6)"}"#;
    LOWER_THIRD
        .replace(
            r##""fill":"#FFFFFF","fit":"shrink""##,
            &format!(r##""fill":"#FFFFFF","fit":"shrink"{shadow}"##),
        )
        .replace(
            r##""fill":"#8FA6BF","fit":"shrink""##,
            &format!(r##""fill":"#8FA6BF","fit":"shrink"{shadow}"##),
        )
}

/// A lower third in a full-frame layer at 4K, holding, draws into a target of its box's class (the card
/// is 60 % of the width and a quarter of the height), not the layer's 33 MB, and with no stencil.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_lower_third_draws_into_its_own_box() {
    let _gpu = gpu_turn().await;
    let (p, registry) = layers(LOWER_THIRD, 1);
    let mut h = Headless::new(registry).await.unwrap();
    reset_effect_target_peak();
    let (_, notes) = scene_frame(&mut h, &Arc::new(p), 1.5, UHD).await;
    let bad: Vec<&String> = notes.iter().filter(|n| *n != "frame.fontMissing").collect();
    assert!(bad.is_empty(), "{notes:?}");
    let stats = h.scene_stats();
    let peak = effect_target_peak();
    assert!(
        peak < UHD_LAYER / 4,
        "its own box, not the layer: {peak} bytes ({:?})",
        stats.bytes
    );
    assert!(
        stats.bytes.buffers < 2 << 20,
        "no stencil, no layer-sized buffer: {:?}",
        stats.bytes
    );
    // Its own target is its card's box (2304 × 562 at 4K) in whole classes, grown for its edges: about a sixth of the
    // layer.
    let card = 2304 * 562 * 4;
    assert!(
        stats.bytes.own >= card && stats.bytes.own < card * 6 / 5,
        "its card's class: {:?}",
        stats.bytes
    );
}

/// Three lower thirds at 4K, moving together and then holding, take a small share of the effect pool (192 MiB),
/// make no GPU object once their classes are surveyed, and are all held while they hold still (three layer-sized
/// targets were past what the pool keeps held: the third was drawn again every frame).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn three_lower_thirds_hold_a_small_share_of_the_pool() {
    let _gpu = gpu_turn().await;
    let (p, registry) = layers(LOWER_THIRD, 3);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    reset_effect_target_peak();
    for t in [0.2, 0.25, 0.3] {
        scene_frame(&mut h, &p, t, UHD).await;
    }
    let before = gpu_objects_made();
    for i in 0..20 {
        scene_frame(&mut h, &p, 0.35 + f64::from(i) * 0.04, UHD).await;
    }
    let made = gpu_objects_made() - before;
    assert_eq!(made, 0, "20 moving frames made {made} GPU objects");
    let peak = effect_target_peak();
    assert!(
        peak < 3 * UHD_LAYER / 4,
        "three boxes, not three layers: {peak} bytes"
    );
    scene_frame(&mut h, &p, 1.5, UHD).await;
    let held = h.scene_stats().held_frames;
    scene_frame(&mut h, &p, 1.6, UHD).await;
    assert_eq!(
        h.scene_stats().held_frames - held,
        3,
        "all three held while they hold still"
    );
}

/// A lower third whose title and role cast shadows draws them in its text pass: nothing offscreen, one pass for
/// the scene beside the frame's own, where each shadow took five passes and a target of its own.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_shadowed_title_and_role_draw_in_one_pass() {
    let _gpu = gpu_turn().await;
    let (p, registry) = layers(&shadowed_lower_third(), 1);
    let mut h = Headless::new(registry).await.unwrap();
    let (img, notes) = scene_frame(&mut h, &Arc::new(p), 1.5, HD).await;
    let bad: Vec<&String> = notes.iter().filter(|n| *n != "frame.fontMissing").collect();
    assert!(bad.is_empty(), "{notes:?}");
    let stats = h.scene_stats();
    assert_eq!(stats.offscreen, 0, "no shadow offscreen: {stats:?}");
    assert_eq!(h.passes_last_frame(), 2, "the frame's pass and the scene's");
    // The shadow shows: darker than the card just below and right of the title's letters.
    let plain = {
        let (p, registry) = layers(LOWER_THIRD, 1);
        let mut h = Headless::new(registry).await.unwrap();
        scene_frame(&mut h, &Arc::new(p), 1.5, HD).await.0
    };
    let darker = img
        .pixels()
        .zip(plain.pixels())
        .filter(|(a, b)| {
            luma_of([a.0[0], a.0[1], a.0[2]]) + 4.0 < luma_of([b.0[0], b.0[1], b.0[2]])
        })
        .count();
    assert!(darker > 500, "the shadows darken the card: {darker} px");
}

/// A lower third's timeline is surveyed on the text build thread, as its text is laid out, so the frame
/// its text lands on packs only itself (the survey took about 0.3 ms there in release, 0.9 ms in debug).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_lower_thirds_survey_is_made_off_its_frame() {
    let _gpu = gpu_turn().await;
    let (p, registry) = layers(LOWER_THIRD, 1);
    let mut h = Headless::new(registry).await.unwrap();
    scene_frame(&mut h, &Arc::new(p), 0.3, HD).await;
    let stats = h.scene_stats();
    assert_eq!(stats.surveys, 0, "none on the frame: {stats:?}");
    assert!(stats.survey_built_us > 0.0, "made with its text: {stats:?}");
}

/// A glass card in a full-frame layer grabs, and blurs, only its own box and its blur's reach of the frame
/// under the layer once its timeline is surveyed (its first frame), not the frame under the whole layer.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_glass_card_grabs_only_its_box() {
    let _gpu = gpu_turn().await;
    let card = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"backdrop","box":[0.05,0.62,0.4,0.26],"radius":24,"blur":16,"tint":"rgba(255, 255, 255, 0.2)"}]}"##;
    let dir = tempfile::tempdir().unwrap();
    let (mut p, registry) = scene_project(card, 2.0);
    stripes_under(&mut p, dir.path());
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    scene_frame(&mut h, &p, 0.5, HD).await;
    let whole = h.scene_stats().bytes.targets;
    let (_, notes) = scene_frame(&mut h, &p, 0.6, HD).await;
    assert!(notes.is_empty(), "{notes:?}");
    let part = h.scene_stats().bytes.targets;
    // Its box: the card (768 × 281) and its blur's reach (three deviations), in whole classes: 880 × 384. The whole
    // layer's copies were blurred halved (a quarter layer each); the box's at its size and half of it.
    let card = 880 * 384 * 4;
    assert!(
        part > 0 && part <= 2 * card && part * 3 < whole * 2,
        "its box's grab and copies, {part} bytes, against the whole layer's {whole}"
    );
}

/// Numbers to compare (run by hand; the gate skips `scene_frame_costs*`): the render thread's time at the start of the
/// frame a 480-letter text lands on (its atlas uploaded on the build thread), at 4K in the preview, and the survey's
/// own time. lavapipe and a debug build: meaningful only against each other.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "run by hand: cargo test --test scene_root_gpu -- --ignored scene_frame_costs_of_a_text_landing"]
async fn scene_frame_costs_of_a_text_landing() {
    let _gpu = gpu_turn().await;
    let heavy = format!(
        r#"{{"scene":1,"duration":2,"nodes":[{{"id":"t","type":"text","text":"{}","split":"words","box":[0,0,1,1],
          "size":120}}]}}"#,
        "ABC DEF GHI JKL MNO PQR STU VWX ".repeat(15).trim_end()
    );
    let (p, registry) = layers(&heavy, 1);
    let p = Arc::new(p);
    let mut h = Headless::preview(registry).await.unwrap();
    let mut landed = 0.0;
    for i in 0..400 {
        scene_frame(&mut h, &p, 0.5, UHD).await;
        let stats = h.scene_stats();
        if stats.land_us > 0.0 {
            landed = stats.land_us;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(if i < 2 {
            50
        } else {
            20
        }))
        .await;
    }
    scene_frame(&mut h, &p, 0.6, UHD).await;
    let stats = h.scene_stats();
    println!(
        "text landing: render thread {landed:.0} µs at the frame's start; text {} bytes; survey {:.0} µs",
        stats.text_bytes, stats.survey_us
    );
}
