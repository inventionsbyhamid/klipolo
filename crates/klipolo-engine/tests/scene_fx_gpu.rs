//! Scene effects on the GPU (lavapipe in CI): text shadows, letters that blur one by one, a span's own
//! gradient, a faded group fading its text as one, emoji particles, a library generator drawing only once checked, a
//! third nested clip and a stroked path at half opacity. Goldens within 2/255.

mod support;
use support::*;

const PANEL: &str = r##"{"type":"rect","box":[0,0,1,1],"fill":"#E8ECF2"}"##;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_title_casts_its_shadow() {
    let _gpu = gpu_turn().await;
    let json = format!(
        r##"{{"scene":1,"duration":2,"nodes":[{PANEL},
      {{"type":"text","text":"Shadow","box":[0.1,0.25,0.8,0.5],"size":150,"weight":800,"fill":"#2D6CDF",
        "align":"center","shadow":{{"x":12,"y":12,"blur":12,"color":"rgba(0, 0, 0, 0.6)"}}}}]}}"##
    );
    let (img, notes) = scene_frame_at(&json, 1.0, None).await;
    assert!(notes.is_empty(), "{notes:?}");
    let grey = |p: [u8; 3]| p[0] < 200 && p[0].abs_diff(p[1]) < 12 && p[1].abs_diff(p[2]) < 12;
    let blue = |p: [u8; 3]| p[2] > 150 && p[0] < 120;
    let shadow = img
        .enumerate_pixels()
        .filter(|(_, _, p)| grey([p.0[0], p.0[1], p.0[2]]))
        .count();
    let ink = img
        .enumerate_pixels()
        .filter(|(_, _, p)| blue([p.0[0], p.0[1], p.0[2]]))
        .count();
    assert!(ink > 2000, "the title draws: {ink} px");
    assert!(shadow > ink / 4, "its shadow shows past it: {shadow} px");
    scene_golden("text-shadow", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn letters_blur_in_one_by_one() {
    let _gpu = gpu_turn().await;
    let json = |blur: u32| {
        format!(
            r##"{{"scene":1,"duration":2,"nodes":[{PANEL},
          {{"id":"t","type":"text","text":"BLURRED","split":"chars","box":[0.05,0.3,0.9,0.4],"size":140,
            "weight":800,"fill":"#14171C","align":"center"}}],
          "timeline":[{{"from":"#t","at":0,"dur":0.5,"blur":{blur},"stagger":0.08}}]}}"##
        )
    };
    let (img, notes) = scene_frame_at(&json(16), 0.35, None).await;
    assert!(notes.is_empty(), "{notes:?}");
    let (sharp, _) = scene_frame_at(&json(0), 0.35, None).await;
    // The letters still blurring spread their ink past where the sharp ones stand.
    let soft =
        |img: &image::RgbaImage| img.pixels().filter(|p| p.0[0] > 60 && p.0[0] < 200).count();
    assert!(
        soft(&img) > soft(&sharp) * 2,
        "blurred letters have soft edges: {} against {}",
        soft(&img),
        soft(&sharp)
    );
    scene_golden("letters-blur-0.35", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_span_gradient_spans_its_own_letters() {
    let _gpu = gpu_turn().await;
    let json = format!(
        r##"{{"scene":1,"duration":2,"nodes":[{PANEL},
      {{"type":"text","box":[0.05,0.3,0.9,0.4],"size":120,"weight":800,"fill":"#14171C","align":"center",
        "spans":[{{"text":"Hot"}},{{"text":"Cold","fill":{{"linear":[0,0,1,0],
          "stops":[[0,"#FF2D55"],[1,"#0A84FF"]]}}}}]}}]}}"##
    );
    let (img, notes) = scene_frame_at(&json, 1.0, None).await;
    assert!(notes.is_empty(), "{notes:?}");
    let colour = |p: &image::Rgba<u8>| p.0[0].max(p.0[2]) > 150 && p.0[1] < 120;
    let xs: Vec<u32> = img
        .enumerate_pixels()
        .filter(|(_, _, p)| colour(p))
        .map(|(x, _, _)| x)
        .collect();
    assert!(
        xs.len() > 500,
        "the span draws in its gradient: {} px",
        xs.len()
    );
    let (lo, hi) = (*xs.iter().min().unwrap(), *xs.iter().max().unwrap());
    let at = |x: u32| {
        (100..260)
            .map(|y| img.get_pixel(x, y).0)
            .find(|p| p[0].max(p[2]) > 150 && p[1] < 120)
    };
    let (first, last) = (at(lo + 6).unwrap(), at(hi - 6).unwrap());
    // Red at the span's own left edge, blue at its right: across its letters, not the whole line's.
    assert!(
        first[0] > first[2] && last[2] > last[0],
        "{first:?} to {last:?}"
    );
    scene_golden("span-gradient", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_faded_group_fades_its_text_as_one() {
    let _gpu = gpu_turn().await;
    let json = r##"{"scene":1,"duration":2,"nodes":[{"type":"rect","box":[0,0,1,1],"fill":"#000000"},
      {"type":"group","box":[0.1,0.2,0.8,0.6],"opacity":0.5,"children":[
        {"type":"rect","box":[0,0,1,1],"fill":"#FF0000"},
        {"type":"text","text":"Fade","box":[0,0,1,1],"size":160,"weight":900,"fill":"#FFFFFF","align":"center"}]}]}"##;
    let (img, notes) = scene_frame_at(json, 1.0, None).await;
    assert!(notes.is_empty(), "{notes:?}");
    // On the letters, white at half over black: never the red card showing through them.
    let letter = img
        .pixels()
        .map(|p| p.0)
        .filter(|p| p[1] > 100)
        .max_by_key(|p| p[1])
        .unwrap();
    assert!(
        letter[0] < 140 && letter[1] > 115,
        "the text at half, alone: {letter:?}"
    );
    scene_golden("faded-group-text", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn emoji_confetti_draws_its_emoji() {
    let _gpu = gpu_turn().await;
    let json = r##"{"scene":1,"duration":2,"nodes":[{"type":"rect","box":[0,0,1,1],"fill":"#101418"},
      {"type":"emitter","box":[0.4,0.3,0.2,0.2],"burst":24,"at":0,"cone":{"angle":-90,"spread":360},
       "speed":[80,240],"life":[2,2],"seed":3,
       "child":{"type":"image","emoji":"🎉","box":[0,0,"60px","60px"]}}]}"##;
    let (img, notes) = scene_frame_at(json, 0.6, None).await;
    let bad: Vec<&String> = notes.iter().filter(|n| *n != "frame.fontMissing").collect();
    assert!(bad.is_empty(), "{notes:?}");
    let lit = img.pixels().filter(|p| p.0[0] > 100).count();
    keep_golden("emoji-confetti", &img);
    assert!(lit > 400, "two dozen emoji draw: {lit} px");
    scene_golden("emoji-confetti", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_library_generator_fill_draws_only_once_checked() {
    let _gpu = gpu_turn().await;
    let json = format!(
        r##"{{"scene":1,"duration":2,"nodes":[{{"type":"rect","box":[0,0,1,1],"fill":"#000000"}},
      {{"type":"rect","box":[0.1,0.1,0.8,0.8],"fill":{{"generator":"{AURORA}"}}}}]}}"##
    );
    let (checked, notes) = scene_frame_at(&json, 1.0, Some(aurora_registry(&[("g", &json)]))).await;
    assert!(notes.is_empty(), "{notes:?}");
    assert!(
        luma_of(rgb_at(&checked, 320, 180)) > 20.0,
        "checked here, it draws"
    );
    let unchecked = unchecked_aurora_registry(&[("g", &json)]);
    let (img, notes) = scene_frame_at(&json, 1.0, Some(unchecked)).await;
    assert!(
        rgb_at(&img, 320, 180).iter().all(|&c| c < 8),
        "not checked: nothing"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("frame.agentEffectChecking") && n.contains(AURORA)),
        "said as being checked: {notes:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_third_nested_clip_still_clips() {
    let _gpu = gpu_turn().await;
    // Three clipped groups, each keeping the middle of the one around it; a white sheet far bigger than all of them.
    let json = r##"{"scene":1,"duration":2,"nodes":[{"type":"rect","box":[0,0,1,1],"fill":"#000000"},
      {"type":"group","box":[0,0,1,1],"clip":"inset(0 50% 0 0)","children":[
        {"type":"group","box":[0,0,1,1],"clip":"inset(25% 0 0 0)","children":[
          {"type":"group","box":[0,0,1,1],"clip":"inset(0 0 50% 0)","children":[
            {"type":"rect","box":[-1,-1,3,3],"fill":"#FFFFFF"}]}]}]}]}"##;
    let (img, notes) = scene_frame_at(json, 1.0, None).await;
    assert!(notes.is_empty(), "{notes:?}");
    let white = |x, y| rgb_at(&img, x, y)[0] > 200;
    // Only the left half, below a quarter and above the middle: every clip holds, the outermost too.
    assert!(white(160, 135), "inside all three");
    assert!(!white(480, 135), "the outer clip holds");
    assert!(!white(160, 40), "the middle clip holds");
    assert!(!white(160, 300), "the inner clip holds");
    scene_golden("three-clips", &img);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_stroked_path_at_half_opacity_shows_no_fill_under_its_stroke() {
    let _gpu = gpu_turn().await;
    let json = r##"{"scene":1,"duration":2,"nodes":[{"type":"rect","box":[0,0,1,1],"fill":"#000000"},
      {"type":"path","box":[0.2,0.2,0.6,0.6],"d":"M0 0 L100 0 L100 100 L0 100 Z","viewBox":[0,0,100,100],
       "preserveAspectRatio":"none","strokeScale":false,"fill":"#FF0000","stroke":"#FFFFFF","strokeWidth":24,
       "opacity":0.5}]}"##;
    let (img, notes) = scene_frame_at(json, 1.0, None).await;
    assert!(notes.is_empty(), "{notes:?}");
    // The stroke's inner half lies over the fill: drawn as one at half opacity, it is grey there, never pink.
    let [r, g, b] = rgb_at(&img, 320, 74);
    assert!(
        r.abs_diff(g) < 12 && g.abs_diff(b) < 12 && g > 100,
        "the stroke over the fill: {:?}",
        [r, g, b]
    );
    let [r, g, _] = rgb_at(&img, 320, 180);
    assert!(r > 110 && g < 20, "the fill alone at half: {:?}", [r, g]);
}
