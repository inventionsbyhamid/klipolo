//! Effect ABI v1.1 on the GPU: a static generator draws once and is reused, a half-size
//! one takes a quarter of the memory and lands where the full-size one does, a stepped one draws once a step, and a
//! split piece draws what the whole layer drew at the same place in the contribution's time.

mod support;
use klipolo_model::contrib::Resolution;
use support::*;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_static_generator_draws_once_and_a_half_size_one_uses_a_quarter_of_the_memory() {
    // Both tests read the process's effect target peak, or make full-size targets that would raise it: one at a time.
    let _gpu = gpu_turn().await;
    let still = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.5, 1.0); }";
    let mut h = headless_with_generator(still, Resolution::Full).await;
    let first = h.render_at(secs(0.1)).await;
    let passes_first = h.passes_last_frame();
    let _ = h.render_at(secs(3.0)).await;
    assert!(
        h.passes_last_frame() < passes_first,
        "the second frame reuses the cached texture"
    );
    let _ = first;
    klipolo_engine::render::effects::reset_effect_target_peak();
    let mut half = headless_with_generator(still, Resolution::Half).await;
    let _ = half.render_at(secs(0.1)).await;
    assert!(klipolo_engine::render::effects::effect_target_peak() <= 960 * 540 * 4 * 2);
}

/// Moves on its steps and draws its layer's randomness: `kl_rand` cells (seeded by `kl.seed` and `kl.frame`) and the
/// seed itself.
const SEEDED: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    let n = kl_rand(floor(uv * 16.0) / 16.0 + vec2<f32>(0.01));
    return vec4<f32>(n, f32(kl.seed % 251u) / 250.0, fract(kl.time), 1.0);
}";

/// Hard-edged stripes: red in every other eighth across, green in every other eighth down.
const STRIPES: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    let r = select(0.0, 1.0, fract(uv.x * 4.0) >= 0.5);
    let g = select(0.0, 1.0, fract(uv.y * 4.0) >= 0.5);
    return vec4<f32>(r, g, 0.5, 1.0);
}";

/// Where `channel` crosses half way along row `y` (`along_x`) or column `y` of a 1920 × 1080 frame: the first pixel
/// past each crossing.
fn edges(rgba: &[u8], y: u32, along_x: bool, channel: usize) -> Vec<u32> {
    let len = if along_x { 1920 } else { 1080 };
    let at = |i: u32| {
        let (x, y) = if along_x { (i, y) } else { (y, i) };
        pixel_at(rgba, (x, y))[channel] >= 128
    };
    (1..len).filter(|&i| at(i) != at(i - 1)).collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn half_size_stepped_and_split_generators_draw_what_they_should() {
    use klipolo_engine::registry::ShaderKind;

    let _gpu = gpu_turn().await;
    // Half size lands where full size does: every stripe edge within 1 px, across and down.
    let full = headless_with_generator(STRIPES, Resolution::Full)
        .await
        .render_at(secs(0.1))
        .await;
    let mut half_frames = headless_with_generator(STRIPES, Resolution::Half).await;
    let half = half_frames.render_at(secs(0.1)).await;
    for (line, along_x, channel) in [
        (300, true, 0),
        (700, true, 0),
        (500, false, 1),
        (1500, false, 1),
    ] {
        let (f, h) = (
            edges(&full.rgba, line, along_x, channel),
            edges(&half.rgba, line, along_x, channel),
        );
        assert_eq!(f.len(), 7, "{f:?}");
        assert!(
            f.len() == h.len() && f.iter().zip(&h).all(|(a, b)| a.abs_diff(*b) <= 1),
            "line {line}: full {f:?}, half {h:?}"
        );
    }
    // The cache's key holds the output size: a new size draws again, then is reused.
    assert_eq!(half_frames.render_at(secs(0.2)).await.passes, 0);
    assert_eq!(
        half_frames
            .render_sized(secs(0.3), (1280, 720))
            .await
            .passes,
        1
    );
    assert_eq!(
        half_frames
            .render_sized(secs(0.4), (1280, 720))
            .await
            .passes,
        0
    );

    // Stepped at 12 fps and split by the editor 2 s in (the pieces have ids of their own): at 2.5 s the
    // second piece draws what the whole layer drew, its seed and its `kl_rand` cells included.
    let def = generator_def("seeded", None, Some(12));
    let registry = registry_of(&[(def.clone(), ShaderKind::Generator, SEEDED)]);
    let project = generator_piece_project(&def, 0.0, 6.0, None, Vec::new());
    let mut split = frames_of(registry.clone(), split_at(&project, "gen", 2.0)).await;
    let mut whole = frames_of(registry, project).await;
    let w = whole.render_at(secs(2.5)).await;
    assert_eq!(w.passes, 1, "the generator's pass");
    assert!(
        w.rgba == split.render_at(secs(2.5)).await.rgba,
        "the second piece continues the whole layer"
    );
    // 2.52 s is still in the step that began at 2.5 s: reused. 2.6 s is the next one: drawn again.
    assert_eq!(whole.render_at(secs(2.52)).await.passes, 0);
    let next = whole.render_at(secs(2.6)).await;
    assert_eq!(next.passes, 1);
    assert!(next.rgba != w.rgba, "a new step draws a new picture");

    // `kl.fps` is the project's rate (60 here) and `kl.step_fps` the generator's (12).
    let rates = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(kl.fps / 60.0, kl.step_fps / 60.0, 0.0, 1.0);
}";
    let def = generator_def("rates", None, Some(12));
    let mut p = generator_piece_project(&def, 0.0, 2.0, None, Vec::new());
    p.settings.fps = 60;
    let mut frames = frames_of(registry_of(&[(def, ShaderKind::Generator, rates)]), p).await;
    let [r, g, _] = pixel_at(&frames.render_at(secs(0.5)).await.rgba, (960, 540));
    assert!(
        r >= 254 && g.abs_diff(51) <= 1,
        "kl.fps {r}/255, kl.step_fps {g}/255"
    );
}
