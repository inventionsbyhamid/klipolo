//! Test helpers for `scene@1` and its renderer: helpers for these go here.

use std::sync::Arc;

use klipolo_engine::registry::Registry;
use klipolo_model::contrib::{Catalog, ContribDef, ContribKind};
use klipolo_model::project::Project;
use klipolo_model::timeline::ItemContent;

use super::{bx, item, overlay_track};

#[path = "scene_text.rs"]
mod text;
// A test that draws no text uses none of these.
#[allow(unused_imports)]
pub use text::*;
#[path = "scene_verified.rs"]
mod verified;
#[allow(unused_imports)]
pub use verified::*;
#[path = "scene_groups.rs"]
mod groups;
#[allow(unused_imports)]
pub use groups::*;

/// A lower third: a card that wipes in through its clip, an accent tab that grows up, a title (`$title`, white, fitted
/// by `shrink`) and a role (blue-grey) that rise and fade in one after the other, and a rule. 4 s, `in` 1 s and `out`
/// 0.5 s: on a longer layer the hold stretches, and the card leaves at `out`. The title is the frame's only white ink
/// (`ink_bounds`); its box is `lower_third_title_box_px` in a full-frame layer of `H_SIZE`.
pub const LOWER_THIRD: &str = r##"{"scene":1,"duration":4,"segments":{"in":1,"out":0.5},
  "params":[{"key":"accent","label":"Accent","type":"color","default":"#FFB000"},
    {"key":"title","label":"Name","type":"text","default":"Ana Lee","maxLen":500},
    {"key":"role","label":"Role","type":"text","default":"Product designer","maxLen":80}],
  "nodes":[{"id":"card","type":"group","box":[0.04,0.62,0.6,0.26],"children":[
    {"id":"plate","type":"rect","box":[0,0,1,1],"fill":"rgba(20, 20, 28, 0.85)","radius":12},
    {"id":"tab","type":"rect","box":[0,0,0.012,1],"fill":"$accent","radius":3,"origin":"50% 100%",
     "animate":{"scaleY":[{"t":0,"v":0},{"t":0.4,"v":1,"ease":"power3.out"}]}},
    {"id":"title","class":"line","type":"text","text":"$title","box":[0.05,0.08,0.9,0.4],"size":48,"weight":700,
     "fill":"#FFFFFF","fit":"shrink"},
    {"id":"role","class":"line","type":"text","text":"$role","box":[0.05,0.58,0.6,0.3],"size":30,"weight":500,
     "fill":"#8FA6BF","fit":"shrink"},
    {"id":"rule","type":"line","box":[0.05,0.52,0.9,0],"stroke":"$accent","strokeWidth":4,"cap":"round"}]}],
  "timeline":[{"set":"#card","clip":"inset(0 100% 0 0)"},
    {"to":"#card","at":0.1,"dur":0.55,"ease":"power3.out","clip":"inset(0 0% 0 0)"},
    {"from":".line","at":0.34,"dur":0.5,"ease":"power3.out","y":"22px","opacity":0,"stagger":0.1},
    {"to":"#card","at":"out","dur":0.35,"ease":"power2.in","y":"18px","opacity":0}]}"##;

/// A number counting from 0 to 10,000 over 1 s (thousands grouped), centred: "10,000" from 1 s.
pub const COUNTER: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"id":"n","type":"text","text":"0","box":[0.1,0.3,0.8,0.4],"size":160,"weight":800,"fill":"#FFFFFF","align":"center",
   "count":{"from":0,"to":10000,"dur":1,"separator":true}}]}"##;

/// "RISE UP", split by letter, each rising 60 px and fading in 0.05 s after the one before.
pub const KINETIC_LETTERS: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"id":"t","type":"text","text":"RISE UP","split":"chars","box":[0.05,0.3,0.9,0.4],"size":150,"weight":800,
   "fill":"#FFD400","align":"center"}],
  "timeline":[{"from":"#t","at":0,"dur":0.4,"y":"60px","opacity":0,"stagger":0.05,"ease":"power3.out"}]}"##;

/// "Make it pop", split by letter, each growing from nothing across in turn; "pop" dark on a yellow marker that grows
/// under it as its letters do.
pub const MARKER: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"id":"m","type":"text","split":"chars","box":[0.05,0.3,0.9,0.4],"size":120,"weight":700,"fill":"#FFFFFF",
   "align":"center","spans":[{"text":"Make it "},
     {"text":"pop","fill":"#111111","background":{"color":"#FFE14D","padding":10,"radius":8}}]}],
  "timeline":[{"from":"#m","at":0.2,"dur":0.5,"scaleX":0,"opacity":0,"stagger":0.06,"ease":"power2.out"}]}"##;

/// One still scene of each basic shape on a light panel: a rounded rect (four different corners) with a soft shadow, a
/// stroked ellipse, a five-point star with a linear gradient, and a rect clipped to its left half.
pub const SHAPES: &str = r##"{"scene":1,"duration":2,"nodes":[
  {"type":"rect","id":"panel","box":[0.02,0.03,0.96,0.94],"fill":"#D8DCE3","radius":24},
  {"type":"rect","id":"card","box":[0.06,0.1,0.38,0.34],"fill":"#2D6CDF","radius":[40,4,16,28],
   "shadow":{"x":12,"y":16,"blur":30,"spread":4,"color":"rgba(0, 0, 0, 0.6)"}},
  {"type":"ellipse","id":"ring","box":[0.56,0.08,0.3,0.4],"fill":"#FFFFFF","stroke":"#E8453C","strokeWidth":12},
  {"type":"star","id":"star","box":[0.08,0.52,0.3,0.42],"sides":5,"innerRadius":0.45,
   "fill":{"linear":[0,0,1,1],"stops":[[0,"#FFD200"],[1,"#FF5E00"]]}},
  {"type":"rect","id":"half","box":[0.55,0.6,0.36,0.3],"fill":"#2BB673","clip":"inset(0 50% 0 0)"}]}"##;

/// A thousand dots in a grid of 40 × 25 whose rows rise and fall: the renderer's heavy common case.
pub const DOT_GRID: &str = r##"{"scene":1,"duration":4,"nodes":[
  {"type":"repeat","id":"rows","times":25,"gap":[0,0.04],"children":[
    {"type":"repeat","times":40,"gap":[0.025,0],"children":[
      {"type":"ellipse","class":"dot","box":[0.005,0.005,0.012,0.02],"fill":"#7FD1FF"}]}]}],
  "timeline":[{"to":".dot","at":0,"dur":1,"y":"12px","yoyo":true,"repeat":-1,"ease":"sine.inOut",
    "stagger":{"each":0.002}}]}"##;

/// The id a test scene `name` has.
pub fn scene_id(name: &str) -> String {
    format!("klipolo.test/scene.{name}")
}

/// The built-ins plus each `(name, scene JSON)`, checked and compiled as a verified scene is.
pub fn scene_registry(scenes: &[(&str, &str)]) -> Arc<Registry> {
    let catalog = Catalog::builtin();
    let compiled = scenes.iter().map(|(name, json)| {
        let def = ContribDef::new(scene_id(name).as_str(), ContribKind::Scene, name);
        let scene = klipolo_engine::registry::compile_scene(json.as_bytes(), catalog)
            .unwrap_or_else(|problems| panic!("{name}: {problems:#?}"));
        (def, scene)
    });
    Arc::new(Registry::builtin().with_scenes(compiled))
}

/// A project of one overlay layer `id` showing `json` from `start` for `duration` seconds over the black background,
/// filling the frame, and the registry that has the scene.
pub fn project_with_scene(
    id: &str,
    json: &str,
    start: f64,
    duration: f64,
) -> (Project, Arc<Registry>) {
    project_with_scene_in(id, json, start, duration, bx(0.0, 0.0, 1.0, 1.0))
}

/// [`project_with_scene`] with the layer in `rect`.
pub fn project_with_scene_in(
    id: &str,
    json: &str,
    start: f64,
    duration: f64,
    rect: klipolo_model::layout::BoxN,
) -> (Project, Arc<Registry>) {
    let registry = scene_registry(&[(id, json)]);
    let mut p = Project::new("Scene");
    p.id = "scene-project".into();
    let content = ItemContent::Generator {
        source: scene_id(id).as_str().into(),
        params: Default::default(),
        still: false,
        offset_us: 0,
        span_us: None,
        seed_from: None,
    };
    p.timeline.tracks.push(overlay_track(
        "titles",
        vec![item(id, start, duration, rect, content)],
    ));
    (p, registry)
}

/// The overlay layer `id`'s item.
pub fn scene_item<'a>(p: &'a mut Project, id: &str) -> &'a mut klipolo_model::timeline::Item {
    p.timeline
        .tracks
        .iter_mut()
        .flat_map(|t| &mut t.items)
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no layer {id}"))
}

/// Layer `id` draws its settled pose (Remove motion on a motion graphic) or moves again.
pub fn set_still(p: &mut Project, id: &str, on: bool) {
    if let ItemContent::Generator { still, .. } = &mut scene_item(p, id).content {
        *still = on;
    }
}

/// The built-ins plus each `(name, scene JSON)` compiled without the check (a scene past the caps the check refuses,
/// which only the runtime caps hold).
pub fn unchecked_scene_registry(scenes: &[(&str, &str)]) -> Arc<Registry> {
    use klipolo_model::scene::{Scene, timeline::compile};
    let compiled = scenes.iter().map(|(name, json)| {
        let def = ContribDef::new(scene_id(name).as_str(), ContribKind::Scene, name);
        let scene = Scene::from_stored(json.as_bytes()).unwrap_or_else(|p| panic!("{p:#?}"));
        (def, Arc::new(compile(&scene)))
    });
    Arc::new(Registry::builtin().with_scenes(compiled))
}

/// A frame `headless` drew of `p` at `at` seconds, landscape, `size` pixels: its pixels and its notes.
pub async fn scene_frame(
    headless: &mut klipolo_engine::headless::Headless,
    p: &Arc<Project>,
    at: f64,
    size: (u32, u32),
) -> (image::RgbaImage, Vec<String>) {
    let landscape = klipolo_model::project::FormatKind::Landscape;
    scene_frame_in(headless, p, at, size, landscape).await
}

/// [`scene_frame`] in `format` (a 9:16 frame is portrait).
pub async fn scene_frame_in(
    headless: &mut klipolo_engine::headless::Headless,
    p: &Arc<Project>,
    at: f64,
    size: (u32, u32),
    format: klipolo_model::project::FormatKind,
) -> (image::RgbaImage, Vec<String>) {
    let f = headless
        .render(p.clone(), super::secs(at), format, size)
        .await
        .unwrap();
    let row = size.0 as usize * 4;
    let rgba: Vec<u8> = f
        .bytes
        .chunks(f.stride as usize)
        .take(size.1 as usize)
        .flat_map(|r| &r[..row])
        .copied()
        .collect();
    (
        image::RgbaImage::from_raw(size.0, size.1, rgba).unwrap(),
        f.warnings,
    )
}

/// `img` against `tests/golden/scene/<name>.png`: every pixel within 2/255 but at most 0.1 % (edges a GPU antialiases its
/// own way). `UPDATE_GOLDEN=1` writes it instead; look at the image before committing it.
pub fn scene_golden(name: &str, img: &image::RgbaImage) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/scene");
    let path = dir.join(format!("{name}.png"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(&dir).unwrap();
        img.save(&path).unwrap();
        return;
    }
    let want = image::open(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (UPDATE_GOLDEN=1 writes it)", path.display()))
        .to_rgba8();
    let (within, mean) = klipolo_engine::test_support::compare(img, &want, 2);
    if let Some(out) = std::env::var_os("GOLDEN_OUT") {
        // The frame drawn and where it differs from the golden, sixteen times over, into that folder.
        let out = std::path::Path::new(&out);
        std::fs::create_dir_all(out).unwrap();
        img.save(out.join(format!("{name}.png"))).unwrap();
        let diff = image::RgbaImage::from_fn(img.width(), img.height(), |x, y| {
            let (a, b) = (img.get_pixel(x, y).0, want.get_pixel(x, y).0);
            let d = |k: usize| (a[k].abs_diff(b[k]) as u32 * 16).min(255) as u8;
            image::Rgba([d(0), d(1), d(2), 255])
        });
        diff.save(out.join(format!("{name}-diff.png"))).unwrap();
    }
    if std::env::var_os("GOLDEN_REPORT").is_some() {
        // How much of it is byte for byte the golden.
        let (exact, _) = klipolo_engine::test_support::compare(img, &want, 0);
        println!(
            "golden {name}: {:.4} % exact, {:.4} % within 2/255, mean {mean:.4}",
            exact * 100.0,
            within * 100.0
        );
    }
    assert!(
        within >= 0.999 && mean <= 0.25,
        "{name}: {:.3} % within 2/255, mean {mean:.3}",
        within * 100.0
    );
}

/// The mean x of the pixels brighter than the background (the frame's ink), in pixels.
pub fn ink_x(img: &image::RgbaImage) -> f64 {
    let (mut sum, mut n) = (0.0f64, 0.0f64);
    for (x, _, p) in img.enumerate_pixels() {
        if p.0[..3].iter().any(|&c| c > 24) {
            sum += f64::from(x);
            n += 1.0;
        }
    }
    sum / n.max(1.0)
}

/// A 64 × 32 checker (red and blue, 8-pixel squares) written into `dir`, as an image asset `id` of `p`, and named by
/// layer `layer`'s setting `key`.
pub fn add_checker_image(p: &mut Project, dir: &std::path::Path, id: &str, layer: &str, key: &str) {
    let path = dir.join(format!("{id}.png"));
    image::RgbaImage::from_fn(64, 32, |x, y| {
        let red = (x / 8 + y / 8) % 2 == 0;
        image::Rgba(if red {
            [230, 60, 60, 255]
        } else {
            [60, 80, 230, 255]
        })
    })
    .save(&path)
    .unwrap();
    p.assets.push(klipolo_model::project::Asset {
        id: id.into(),
        kind: klipolo_model::project::AssetKind::Image,
        path: path.to_string_lossy().into_owned(),
        name: format!("{id}.png"),
        duration_us: 0,
        width: 64,
        height: 32,
        fps: 0.0,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    });
    if let ItemContent::Generator { params, .. } = &mut scene_item(p, layer).content {
        params.insert(
            key.into(),
            klipolo_model::params::ParamValue::Text(id.into()),
        );
    }
}

/// The pixel at `(x, y)`, RGB.
pub fn rgb_at(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = img.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}
