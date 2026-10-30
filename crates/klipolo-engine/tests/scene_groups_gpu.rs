//! Scene groups on the GPU (lavapipe in CI): offscreen groups (blur, blend, masks, a 2.5D tilt),
//! glass backdrops over what is under the layer, generator fills, seeded particles and repeats over a table's rows, at
//! most four groups offscreen at once. Goldens are compared within 2/255; `UPDATE_GOLDEN=1` writes them.

mod support;
use std::sync::Arc;

use klipolo_engine::headless::Headless;
use support::*;

/// A light panel, a blue column, a star blurred by its group and a yellow disc multiplied over the column.
const BLUR_MULTIPLY: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"type":"rect","box":[0,0,1,1],"fill":"#E8ECF2"},
  {"type":"rect","box":[0.55,0.1,0.25,0.8],"fill":"#2D6CDF"},
  {"type":"group","id":"soft","box":[0.05,0.15,0.4,0.7],"blur":12,"children":[
    {"type":"star","box":[0.1,0.05,0.8,0.9],"sides":5,"fill":"#FF5E00"}]},
  {"type":"group","id":"mul","box":[0.5,0.25,0.4,0.5],"blend":"multiply","children":[
    {"type":"ellipse","box":[0.1,0,0.8,1],"fill":"#FFD200"}]}]}"##;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_blurred_group_and_a_multiply_group() {
    let _gpu = gpu_turn().await;
    let (img, notes) = scene_frame_at(BLUR_MULTIPLY, 1.0, None).await;
    assert!(notes.is_empty(), "{notes:?}");
    // The star's edges blur into the panel: no sharp step between neighbouring pixels across it (drawn sharp, the step
    // from panel to orange is about a hundred).
    let step = (60..300)
        .flat_map(|y| (20..300).map(move |x| (x, y)))
        .map(|(x, y)| (luma_of(rgb_at(&img, x, y)) - luma_of(rgb_at(&img, x + 1, y))).abs())
        .fold(0.0, f64::max);
    assert!(
        step < 30.0,
        "the blurred star has no sharp edge: a step of {step:.0}"
    );
    // Yellow multiplied over blue is dark green; over the panel it stays a light yellow.
    let [r, g, b] = rgb_at(&img, 430, 180);
    assert!(
        r < 70 && g > 60 && g < 140 && b < 40,
        "yellow × blue: {:?}",
        [r, g, b]
    );
    let [r, g, b] = rgb_at(&img, 530, 180);
    assert!(
        r > 200 && g > 160 && b < 40,
        "yellow × panel: {:?}",
        [r, g, b]
    );
    scene_golden("groups-blur-multiply", &img);
}

/// A gradient shown only through a hidden circle (alpha), and a blue column shown where a hidden black-to-white ramp
/// is dark (luma, inverted).
const MASKS: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"type":"rect","box":[0,0,1,1],"fill":"#202020"},
  {"type":"ellipse","id":"spot","box":[0.1,0.2,0.3,0.6],"fill":"#FFFFFF","opacity":0},
  {"type":"group","box":[0,0,1,1],"mask":{"node":"#spot"},"children":[
    {"type":"rect","box":[0.05,0.1,0.4,0.8],"fill":{"linear":[0,0,1,0],"stops":[[0,"#FF3B30"],[1,"#34C759"]]}}]},
  {"type":"rect","id":"ramp","box":[0.55,0.1,0.4,0.8],
   "fill":{"linear":[0,0,1,0],"stops":[[0,"#000000"],[1,"#FFFFFF"]]},"opacity":0},
  {"type":"group","box":[0,0,1,1],"mask":{"node":"#ramp","mode":"luma","invert":true},"children":[
    {"type":"rect","box":[0.55,0.1,0.4,0.8],"fill":"#0A84FF"}]}]}"##;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn alpha_and_inverted_luma_masks() {
    let _gpu = gpu_turn().await;
    let (img, notes) = scene_frame_at(MASKS, 1.0, None).await;
    assert!(notes.is_empty(), "{notes:?}");
    let dark = |p: [u8; 3]| p.iter().all(|&c| c < 50);
    // Inside the circle the gradient shows; inside its rect but outside the circle, only the background.
    assert!(
        !dark(rgb_at(&img, 160, 180)),
        "the circle shows the gradient"
    );
    assert!(
        dark(rgb_at(&img, 50, 60)),
        "outside the circle: {:?}",
        rgb_at(&img, 50, 60)
    );
    // The inverted luma mask: full blue where the ramp is black (left), none where it is white (right).
    let left = rgb_at(&img, 360, 180);
    let right = rgb_at(&img, 600, 180);
    assert!(
        left[2] > 200 && left[0] < 40,
        "blue at the ramp's dark end: {left:?}"
    );
    assert!(dark(right), "nothing at the ramp's light end: {right:?}");
    scene_golden("masks", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_tilted_group_has_perspective() {
    let _gpu = gpu_turn().await;
    const TILT: &str = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"rect","box":[0,0,1,1],"fill":"#101418"},
      {"type":"group","box":[0.25,0.15,0.5,0.7],"rotateY":35,"perspective":800,"children":[
        {"type":"rect","box":[0,0,1,1],"fill":"#F2C94C","radius":16,"stroke":"#FFFFFF","strokeWidth":6}]}]}"##;
    let (img, notes) = scene_frame_at(TILT, 1.0, None).await;
    assert!(notes.is_empty(), "{notes:?}");
    let lit = |x: u32| {
        (0..360)
            .filter(|&y| luma_of(rgb_at(&img, x, y)) > 80.0)
            .count()
    };
    let cols: Vec<u32> = (0..640).filter(|&x| lit(x) > 0).collect();
    let (first, last) = (*cols.first().unwrap(), *cols.last().unwrap());
    // Turned away on the right: the near (left) edge stands taller than the far one, and the card is narrower.
    let (near, far) = (lit(first + 4), lit(last - 4));
    assert!(
        near > far + 20,
        "perspective: near edge {near} px, far edge {far} px"
    );
    assert!(
        last - first < 320,
        "narrower than its 320 px: {}",
        last - first
    );
    scene_golden("tilt", &img);
}

/// A glass card: a rounded backdrop blurring and lightening what is under the layer, over a pale outline.
const GLASS: &str = r##"{"scene":1,"duration":4,"segments":{"in":1,"out":1},"nodes":[
  {"type":"backdrop","id":"glass","box":[0.25,0.25,0.5,0.5],"radius":24,"blur":24,"tint":"rgba(255, 255, 255, 0.25)"},
  {"type":"rect","box":[0.25,0.25,0.5,0.5],"radius":24,"fill":"rgba(0, 0, 0, 0)","stroke":"rgba(255, 255, 255, 0.6)",
   "strokeWidth":2}]}"##;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_glass_card_blurs_what_is_under_it() {
    let _gpu = gpu_turn().await;
    let dir = tempfile::tempdir().unwrap();
    let (mut p, registry) = scene_project(GLASS, 4.0);
    stripes_under(&mut p, dir.path());
    let mut h = Headless::new(registry).await.unwrap();
    let (img, notes) = scene_frame(&mut h, &Arc::new(p), 2.0, G_SIZE).await;
    keep_golden("glass", &img);
    assert!(notes.is_empty(), "{notes:?}");
    let spread = |y: u32, xs: std::ops::Range<u32>| {
        let l: Vec<f64> = xs.map(|x| luma_of(rgb_at(&img, x, y))).collect();
        l.iter().cloned().fold(f64::MIN, f64::max) - l.iter().cloned().fold(f64::MAX, f64::min)
    };
    // Outside the card the stripes are sharp; under it they blur to a lightened grey.
    assert!(
        spread(40, 100..140) > 200.0,
        "sharp outside: {}",
        spread(40, 100..140)
    );
    // Right up to its edge: the card itself isn't blurred (its `blur` is what it shows).
    assert!(
        spread(86, 280..360) > 200.0,
        "sharp just outside: {}",
        spread(86, 280..360)
    );
    assert!(
        spread(96, 280..360) < 40.0,
        "blurred just inside: {}",
        spread(96, 280..360)
    );
    assert!(
        spread(180, 280..360) < 40.0,
        "blurred inside: {}",
        spread(180, 280..360)
    );
    assert!(
        luma_of(rgb_at(&img, 320, 180)) > 140.0,
        "tinted lighter inside"
    );
    scene_golden("glass", &img);
}

/// Inside the scene's hold, the card over video whose frames change redraws every frame, its pixels following the
/// video, and nothing is held.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_glass_card_over_moving_video_redraws_during_its_hold() {
    let _gpu = gpu_turn().await;
    let (mut p, registry) = scene_project(GLASS, 4.0);
    video_under(&mut p);
    let p = Arc::new(p);
    let mut h = Headless::new(registry).await.unwrap();
    let mut seen = Vec::new();
    for t in [1.5, 2.0, 2.5] {
        let (img, notes) = scene_frame(&mut h, &p, t, G_SIZE).await;
        assert!(notes.is_empty(), "{notes:?}");
        seen.push(luma_of(rgb_at(&img, 320, 180)));
        assert_eq!(h.scene_stats().held_frames, 0, "never held at {t}");
    }
    for w in seen.windows(2) {
        assert!(
            (w[0] - w[1]).abs() > 20.0,
            "the card follows the video: {seen:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_aurora_filled_rect() {
    let _gpu = gpu_turn().await;
    let scene = format!(
        r##"{{"scene":1,"duration":2,"nodes":[
      {{"type":"rect","box":[0,0,1,1],"fill":"#0B0F14"}},
      {{"type":"rect","box":[0.05,0.1,0.55,0.8],"radius":28,"stroke":"#FFFFFF","strokeWidth":4,
        "fill":{{"generator":"{AURORA}","params":{{"speed":1}}}}}},
      {{"type":"ellipse","box":[0.66,0.25,0.3,0.5],
        "fill":{{"generator":"klipolo.builtin/generator.gradient",
          "params":{{"startColor":"#FF0080","endColor":"#00C8FF","angle":0}}}}}}]}}"##
    );
    let registry = aurora_registry(&[("g", &scene)]);
    let (img, notes) = scene_frame_at(&scene, 1.0, Some(registry)).await;
    assert!(notes.is_empty(), "{notes:?}");
    // The aurora fills its rounded rect, never past its corner; the gradient runs pink to blue across the ellipse.
    let inside = rgb_at(&img, 190, 180);
    assert!(
        inside.iter().any(|&c| c > 40),
        "the aurora shows: {inside:?}"
    );
    assert!(
        rgb_at(&img, 33, 37).iter().all(|&c| c < 30),
        "outside its corner"
    );
    let (left, right) = (rgb_at(&img, 440, 180), rgb_at(&img, 600, 180));
    assert!(
        left[0] > left[2] && right[2] > right[0],
        "pink to blue: {left:?} {right:?}"
    );
    scene_golden("aurora", &img);
}

/// Confetti: a seeded burst of turning, falling squares.
fn confetti(seed: u32) -> String {
    format!(
        r##"{{"scene":1,"duration":2,"nodes":[{{"type":"emitter","box":[0.45,0.2,0.1,0.1],"burst":60,"at":0.2,
      "cone":{{"angle":-90,"spread":120}},"speed":[180,420],"gravity":600,"spin":[-360,360],"life":[1.2,1.6],
      "seed":{seed},"child":{{"type":"rect","box":[0,0,"24px","14px"],"fill":"#FFC93C"}}}}]}}"##
    )
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn confetti_lands_in_the_same_place_every_render() {
    let _gpu = gpu_turn().await;
    let (a, notes) = scene_frame_at(&confetti(7), 0.8, None).await;
    assert!(notes.is_empty(), "{notes:?}");
    let (b, _) = scene_frame_at(&confetti(7), 0.8, None).await;
    let (c, _) = scene_frame_at(&confetti(8), 0.8, None).await;
    let lit = a.pixels().filter(|p| p.0[0] > 120).count();
    assert!(lit > 200, "the confetti shows: {lit} px");
    assert!(
        a.as_raw() == b.as_raw(),
        "the same seed lands in the same place"
    );
    assert!(a.as_raw() != c.as_raw(), "another seed lands elsewhere");
}

/// Three bars over a table's rows, each `$row.v0` tall, growing up one after the other.
const BARS: &str = r##"{"scene":1,"duration":2,
  "params":[{"key":"data","label":"Data","type":"table","columns":["v"],"maxRows":8,"default":[
    {"label":"a","values":[0.4]},{"label":"b","values":[0.9]},{"label":"c","values":[0.6]}]}],
  "nodes":[{"type":"rect","box":[0,0,1,1],"fill":"#14171C"},
    {"type":"repeat","rows":"$data","gap":[0.25,0],"children":[
      {"type":"rect","class":"bar","box":[0.15,0.1,0.15,0.8],"origin":"50% 100%","scaleY":"$row.v0",
       "fill":"#4FD1C5","radius":6}]}],
  "timeline":[{"from":".bar","at":0,"dur":0.6,"scaleY":0,"stagger":0.2,"ease":"power2.out"}]}"##;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_bar_chart_grows_from_its_rows() {
    let _gpu = gpu_turn().await;
    let height =
        |img: &image::RgbaImage, x: u32| (0..360).filter(|&y| rgb_at(img, x, y)[1] > 150).count();
    let (grown, notes) = scene_frame_at(BARS, 1.8, None).await;
    assert!(notes.is_empty(), "{notes:?}");
    let hs = [146, 306, 466].map(|x| height(&grown, x));
    // Grown: 0.4, 0.9 and 0.6 of their 288 px.
    for (h, v) in hs.iter().zip([0.4, 0.9, 0.6]) {
        assert!((*h as f64 - 288.0 * v).abs() < 4.0, "bars {hs:?}");
    }
    let (growing, _) = scene_frame_at(BARS, 0.3, None).await;
    let early = [146, 306, 466].map(|x| height(&growing, x));
    assert!(
        early[0] > early[2],
        "the first bar is ahead of the last: {early:?}"
    );
    scene_golden("bars-0.3", &growing);
}
