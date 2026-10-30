//! Padding on the GPU: a padded filter draws outside its layer's box, a text or
//! generator layer needs no extra pass for it, and the margin stops at the frame.

mod support;
use support::*;

/// A "halo" filter: white wherever the layer is transparent within 12 px of opaque pixels.
const HALO: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    let here = kl_sample_a(uv);
    if (here.a > 0.0) { return here; }
    let r = 12.0 * kl.px / kl.size;
    let near = max(max(kl_sample_a(uv + vec2<f32>(r.x, 0.0)).a, kl_sample_a(uv - vec2<f32>(r.x, 0.0)).a),
                   max(kl_sample_a(uv + vec2<f32>(0.0, r.y)).a, kl_sample_a(uv - vec2<f32>(0.0, r.y)).a));
    return vec4<f32>(near);
}";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn padding_lets_a_filter_draw_outside_the_box() {
    // The two tests read the process's effect target peak: one at a time.
    let _gpu = gpu_turn().await;
    // A 100 × 100 px red image at (400, 300) in a 1920 × 1080 frame, black background.
    let outside = (395, 350); // 5 px left of the box
    let without = render_image_with_filter(HALO, None).await;
    assert_eq!(pixel_at(&without, outside), [0, 0, 0]);
    let with = render_image_with_filter(HALO, Some(16.0)).await;
    assert_eq!(pixel_at(&with, outside), [255, 255, 255]);
    // The grown targets are the layer's own: two of 132 × 132 × 4 bytes at most.
    assert!(klipolo_engine::render::effects::effect_target_peak() <= 2 * 132 * 132 * 4);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_padded_generator_needs_no_extra_pass_and_the_margin_stops_at_the_frame() {
    let _gpu = gpu_turn().await;
    // A full-frame generator with the halo filter padded by 16: the margin lies outside the canvas, so nothing grows.
    let full = render_generator_with_filter(HALO, Some(16.0), full_box()).await;
    assert_eq!(full.passes, 2, "the generator and the halo, no PAD pass");
    assert!(klipolo_engine::render::effects::effect_target_peak() <= 2 * 1920 * 1080 * 4);
    // A 100 × 100 generator: still no PAD pass, and the halo reaches outside its box.
    let small = render_generator_with_filter(HALO, Some(16.0), bx_px(400, 300, 100, 100)).await;
    assert_eq!(small.passes, 2);
    assert_eq!(pixel_at(&small.rgba, (395, 350)), [255, 255, 255]);
}

/// A pass-through filter, padded or not by its definition.
const PASS: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return kl_sample_a(uv); }";

/// A grey drop shadow 8 px down and right of whatever the layer covers.
const SHADOW: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    let here = kl_sample_a(uv);
    let shade = kl_sample_a(uv - vec2<f32>(8.0) * kl.px / kl.size).a;
    return here + vec4<f32>(0.5, 0.5, 0.5, 1.0) * shade * (1.0 - here.a);
}";

fn near(a: [u8; 3], b: [u8; 3], tolerance: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tolerance)
}

/// A padded image zooming 1 → 0.01 → 1 over 60 frames reuses its grown targets: after the first frame no
/// more than two grown sizes are made (no GPU objects beyond them), and the targets never pass the margin's cap.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_zooming_padded_layer_reuses_its_grown_targets() {
    use klipolo_engine::render::effects::{
        effect_target_peak, gpu_objects_made, reset_effect_target_peak,
    };
    use klipolo_model::motion::{Key, Motion, MotionKeys};
    let _gpu = gpu_turn().await;
    let dir = tempfile::tempdir().unwrap();
    let registry = test_registry(&[("halo", HALO, Some(16.0))]);
    let mut p = image_project(dir.path(), [255, 0, 0], (400, 300, 100), vec![fx("halo")]);
    let key = |at: f64, v: f64| Key {
        t_us: secs(at),
        v,
        ease: None,
    };
    p.timeline.tracks[0].items[0].layout.motion = Some(Motion {
        keys: Some(MotionKeys {
            scale: vec![key(0.0, 1.0), key(1.0, 0.01), key(2.0, 1.0)],
            ..Default::default()
        }),
        ..Default::default()
    });
    let p = std::sync::Arc::new(p);
    let mut headless = klipolo_engine::headless::Headless::new(registry)
        .await
        .unwrap();
    // 960 × 540: the 100 px image is 50 px, its margin 8 px, so its grown target is 66² at most.
    let size = (960, 540);
    render_on(&mut headless, p.clone(), 0, size).await;
    let made = gpu_objects_made();
    reset_effect_target_peak();
    for k in 1..60 {
        render_on(&mut headless, p.clone(), secs(f64::from(k) / 30.0), size).await;
    }
    assert!(
        gpu_objects_made() - made <= 4,
        "{} GPU objects made while it zoomed",
        gpu_objects_made() - made
    );
    assert!(
        effect_target_peak() <= 2 * 66 * 66 * 4,
        "{}",
        effect_target_peak()
    );
}

/// A rounded card with a drop shadow keeps its round corners, and its shadow falls outside its box.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_rounded_card_with_a_drop_shadow_keeps_its_corners() {
    let _gpu = gpu_turn().await;
    let registry = test_registry(&[("shadow", SHADOW, Some(16.0))]);
    // 200 × 200 px at (400, 300), corners a quarter of its side (50 px).
    let card = generator_project(bx_px(400, 300, 200, 200), 0.25, vec![fx("shadow")]);
    let f = render(registry, card, (1920, 1080)).await;
    assert_eq!(pixel_at(&f.rgba, (500, 400)), [255, 255, 255], "the card");
    assert_eq!(
        pixel_at(&f.rgba, (402, 302)),
        [0, 0, 0],
        "outside its round corner"
    );
    let shadow = pixel_at(&f.rgba, (604, 400));
    assert!(
        near(shadow, [128, 128, 128], 2),
        "its shadow past its right edge: {shadow:?}"
    );
}

/// A circle facecam (a 200 px image under the shape mask) keeps its circle with a 16 px glow, whether the
/// glow comes after the mask (the mask runs at the layer's own size) or before it (the mask measures the layer's own
/// box in the grown target).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_circle_facecam_with_a_glow_keeps_its_circle() {
    let _gpu = gpu_turn().await;
    let dir = tempfile::tempdir().unwrap();
    let mask = || fx(klipolo_model::contrib::builtin::EFFECT_MASK);
    for (order, stack) in [
        ("mask, glow", vec![mask(), fx("halo")]),
        ("glow, mask", vec![fx("halo"), mask()]),
    ] {
        let registry = test_registry(&[("halo", HALO, Some(16.0))]);
        let p = image_project(dir.path(), [255, 0, 0], (400, 300, 200), stack);
        let f = render(registry, p, (1920, 1080)).await;
        assert_eq!(
            pixel_at(&f.rgba, (500, 400)),
            [255, 0, 0],
            "{order}: its centre"
        );
        // 114 px from its centre: outside its 100 px circle, beyond the glow's 12 px, inside a 116 px one.
        assert_eq!(
            pixel_at(&f.rgba, (419, 319)),
            [0, 0, 0],
            "{order}: outside its circle"
        );
        if order == "mask, glow" {
            assert_eq!(
                pixel_at(&f.rgba, (500, 295)),
                [255, 255, 255],
                "{order}: the glow over its top"
            );
        }
    }
}

/// A vignette after a padded filter stays centred on the layer while the margin is cut at a frame edge
/// (left margin 4 px, right 16): its shading is the same either side of the layer, and the same as in mid-frame.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_vignette_stays_centred_as_the_margin_is_cut_at_a_frame_edge() {
    use klipolo_model::params::ParamValue;
    let _gpu = gpu_turn().await;
    let dir = tempfile::tempdir().unwrap();
    let vignette = fx(klipolo_model::contrib::builtin::EFFECT_VIGNETTE)
        .with("amount", ParamValue::Number(1.0))
        .with("softness", ParamValue::Number(1.0));
    let mut shades = Vec::new();
    for x in [4, 400] {
        let registry = test_registry(&[("halo", HALO, Some(16.0))]);
        let stack = vec![fx("halo"), vignette.clone()];
        let p = image_project(dir.path(), [255, 255, 255], (x, 300, 100), stack);
        let f = render(registry, p, (1920, 1080)).await;
        let (left, right) = (
            pixel_at(&f.rgba, (x + 10, 350)),
            pixel_at(&f.rgba, (x + 89, 350)),
        );
        assert!(near(left, right, 3), "at x {x}: {left:?} against {right:?}");
        shades.push(left);
    }
    assert!(near(shades[0], shades[1], 3), "{shades:?}");
}

/// A side with no margin left (an image flush with the frame's left edge) keeps the edge an unpadded layer
/// has: PAD leaves it to the quad, so it isn't faded twice.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_layer_flush_with_the_frame_keeps_its_edge() {
    let _gpu = gpu_turn().await;
    let dir = tempfile::tempdir().unwrap();
    let mut edges = Vec::new();
    for pad in [None, Some(16.0)] {
        let registry = test_registry(&[("pass", PASS, pad)]);
        let p = image_project(dir.path(), [255, 255, 255], (0, 300, 100), vec![fx("pass")]);
        let f = render(registry, p, (1920, 1080)).await;
        edges.push([pixel_at(&f.rgba, (0, 350)), pixel_at(&f.rgba, (99, 350))]);
    }
    for side in 0..2 {
        assert!(near(edges[0][side], edges[1][side], 2), "{edges:?}");
    }
}

/// A padded filter on a title, whose texture is cut to its ink and
/// whose effect target grows by the filter's margin: the shadow falls past the ink's right and bottom edges, as far as
/// the filter casts it, and the ink itself isn't cut.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_padded_filter_on_a_title_shades_past_its_ink() {
    let _gpu = gpu_turn().await;
    let registry = test_registry(&[("shadow", SHADOW, Some(16.0))]);
    let mut p = text_project("Inter", 900, "H");
    let item = &mut p.timeline.tracks[0].items[0];
    item.effects = vec![fx("shadow")];
    // The title's own soft shadow off: only the filter's shades past the ink.
    if let klipolo_model::timeline::ItemContent::Text { style, .. } = &mut item.content {
        style.shadow = false;
    }
    let f = render(registry, p, (1920, 1080)).await;
    // The ink's box: its white pixels.
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..1080 {
        for x in 0..1920 {
            if pixel_at(&f.rgba, (x, y)).iter().all(|c| *c >= 250) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    assert!(x1 > x0 && y1 > y0, "the title's ink");
    // Inside the H's left stem, 4 px in once shifted by the cast (8 px down and right).
    let (mid_x, mid_y) = (x0 + 12, (y0 + y1) / 2);
    let right = pixel_at(&f.rgba, (x1 + 5, mid_y));
    let below = pixel_at(&f.rgba, (mid_x, y1 + 5));
    assert!(
        near(right, [128, 128, 128], 12),
        "the shadow past its right edge: {right:?}"
    );
    assert!(
        near(below, [128, 128, 128], 12),
        "the shadow past its bottom edge: {below:?}"
    );
    assert_eq!(
        pixel_at(&f.rgba, (x1 + 12, mid_y)),
        [0, 0, 0],
        "nothing past the cast"
    );
}
