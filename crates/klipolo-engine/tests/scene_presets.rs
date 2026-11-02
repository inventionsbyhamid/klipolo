//! The shipped motion graphic presets: the lower third (`klipolo.builtin/scene.lower-third`: name, role and accent) and
//! the centred title card (`klipolo.builtin/scene.title-card`: title, subtitle and accent), data folders under
//! `klipolo-model/presets/` in exactly the shape a library version has (`contrib.json` beside `scene.json`). Built-ins,
//! so the preview's registry and the export worker's both draw them; each fills the frame.
//! - each is a scene an agent could have made: its definition's settings are the scene's own, and it checks;
//! - the title card's settings bind: the project keeps them, the plan hands them over, and its nodes show them;
//! - drawn (lavapipe in CI): the lower third's title area shows its name in the preview and in an exported file, and
//!   the title card its title and accent; in 16:9, 9:16 and 1:1 both keep their shape (their heights are 1080p pixels,
//!   not shares of the frame) and in 9:16 everything either draws stays inside the area the apps keep clear
//!   (`PORTRAIT_CLEAR`); an empty role or subtitle shows nothing; animated words, the scene their settings make, come in
//!   word by word inside their own box. `PRESET_STILLS=<folder>` keeps a still of each.

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use klipolo_engine::export::contrib::job_registry;
use klipolo_engine::headless::Headless;
use klipolo_engine::plan::{Layer, PlanContext, frame_plan};
use klipolo_engine::registry::Registry;
use klipolo_engine::registry::pack::{check_definition, names_clean, setting_text_clean};
use klipolo_model::contrib::{ContribId, ContribKind};
use klipolo_model::layout::Layout;
use klipolo_model::media::text::PORTRAIT_CLEAR;
use klipolo_model::params::{ParamKind, ParamValue};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::scene::Scene;
use klipolo_model::scene::eval::{SceneFrame, eval_into};
use klipolo_model::scene::instance::SceneInstance;
use klipolo_model::timeline::ItemContent;
use support::*;

const LOWER_THIRD: &str = "scene.lower-third";
const TITLE_CARD: &str = "scene.title-card";
const KINETIC_TITLE: &str = "scene.kinetic-title";
/// Where the stills are drawn: landscape, 640 px across.
const SIZE: (u32, u32) = (640, 360);

/// Preset `local`'s folder.
fn folder(local: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../klipolo-model/presets")
        .join(local)
}

/// Preset `local`'s `scene.json`, as text and read.
fn scene_json(local: &str) -> (String, Scene) {
    let path = folder(local).join("scene.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let scene = Scene::from_stored(text.as_bytes()).unwrap_or_else(|p| panic!("{local}: {p:#?}"));
    (text, scene)
}

/// A project of one layer `layer` showing preset `local` from 0 for `seconds`, with `params`, placed as every template
/// is: `Layout::full_frame`, so it fills the frame in every format (derived from 16:9 it would be a square in 9:16).
fn project(local: &str, seconds: f64, params: &[(&str, &str)]) -> Project {
    let mut p = Project::new("Presets");
    let values: BTreeMap<String, ParamValue> = params
        .iter()
        .map(|(k, v)| (k.to_string(), ParamValue::Text(v.to_string())))
        .collect();
    let content = ItemContent::generator(ContribId::builtin(local), values);
    let mut layer = item("layer", 0.0, seconds, bx(0.0, 0.0, 1.0, 1.0), content);
    layer.layout = Layout::full_frame();
    p.timeline.tracks.push(overlay_track("titles", vec![layer]));
    p
}

/// Node `id`'s box in `scene.json`, in the pixels of a full-frame layer of `size`: each box inside its parent's, a
/// number a fraction of the parent's side and `"Npx"` 1080p pixels (the frame's shorter side is 1080 of them), then
/// moved by the node's own `x` and `y` (pixels only here).
fn node_px(json: &str, id: &str, size: (u32, u32)) -> Bounds {
    /// A box value: a fraction of `side`, or `"Npx"`, `k` frame pixels each.
    fn length(v: &serde_json::Value, side: f64, k: f64) -> f64 {
        match v.as_str() {
            Some(px) => px.strip_suffix("px").unwrap().parse::<f64>().unwrap() * k,
            None => v.as_f64().unwrap() * side,
        }
    }
    fn walk(nodes: &[serde_json::Value], id: &str, outer: [f64; 4], k: f64) -> Option<[f64; 4]> {
        for n in nodes {
            let mut here = match n["box"].as_array() {
                Some(b) => [
                    outer[0] + length(&b[0], outer[2], k),
                    outer[1] + length(&b[1], outer[3], k),
                    length(&b[2], outer[2], k),
                    length(&b[3], outer[3], k),
                ],
                None => outer,
            };
            for (axis, key) in [(0, "x"), (1, "y")] {
                if let Some(v) = n.get(key).filter(|v| v.is_string()) {
                    here[axis] += length(v, 0.0, k);
                }
            }
            if n["id"] == id {
                return Some(here);
            }
            if let Some(children) = n["children"].as_array()
                && let Some(found) = walk(children, id, here, k)
            {
                return Some(found);
            }
        }
        None
    }
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    let k = f64::from(size.0.min(size.1)) / 1080.0;
    let frame = [0.0, 0.0, f64::from(size.0), f64::from(size.1)];
    let [x, y, w, h] = walk(v["nodes"].as_array().unwrap(), id, frame, k)
        .unwrap_or_else(|| panic!("no node {id}"));
    Bounds {
        x0: x,
        y0: y,
        x1: x + w,
        y1: y + h,
    }
}

/// `img` into `$PRESET_STILLS/<name>.png` when that is set.
fn keep_still(name: &str, img: &image::RgbaImage) {
    if let Some(dir) = std::env::var_os("PRESET_STILLS") {
        let dir = Path::new(&dir);
        std::fs::create_dir_all(dir).unwrap();
        img.save(dir.join(format!("{name}.png"))).unwrap();
    }
}

/// The text instance node `id` shows in `f`.
fn text_of<'a>(inst: &'a SceneInstance, f: &'a SceneFrame, id: &str) -> &'a str {
    let c = inst
        .compiled()
        .find(id)
        .unwrap_or_else(|| panic!("no node {id}"));
    let node = inst.instances_of(c).next().unwrap();
    f.text(inst, node)
}

#[test]
fn every_scene_preset_is_one_an_agent_could_have_made() {
    let reg = Registry::builtin();
    for (local, keys) in [
        (LOWER_THIRD, &["title", "role", "accent"][..]),
        (TITLE_CARD, &["title", "subtitle", "accent"][..]),
        (KINETIC_TITLE, &["title"][..]),
    ] {
        let id = ContribId::builtin(local);
        assert!(
            reg.scene(&id).is_some(),
            "{local}: the built-in registry draws it"
        );
        let def = reg.catalog().get(&id).unwrap().clone();
        assert_eq!(def.kind, ContribKind::Scene, "{local}");
        let (text, scene) = scene_json(local);
        // A stored scene's definition carries the scene's own settings (`scene.paramsDiffer` otherwise).
        assert_eq!(def.params, scene.params, "{local}");
        let have: Vec<&str> = def.params.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(have, keys, "{local}");
        for spec in &def.params {
            match (&spec.kind, spec.key.as_str()) {
                (ParamKind::Color { default, .. }, "accent") => {
                    assert_eq!(default, "#FFD02E", "{local}: the brand yellow")
                }
                (ParamKind::Text { default, .. }, _) => assert!(!default.is_empty(), "{local}"),
                (kind, key) => panic!("{local}: {key} is {kind:?}"),
            }
        }
        klipolo_model::scene::check_scene(&scene, reg.catalog())
            .unwrap_or_else(|p| panic!("{local}: {p:#?}"));
        let mut as_agent = def.clone();
        as_agent.id = format!("lib.k3f9q2ma/{local}.v1").as_str().into();
        names_clean(&as_agent).unwrap_or_else(|f| panic!("{local}: its {f} isn't clean"));
        setting_text_clean(&as_agent).unwrap_or_else(|k| panic!("{local}: setting {k}"));
        check_definition(&as_agent).unwrap_or_else(|e| panic!("{local}: {e}"));
        // Copy has no em or en dashes.
        assert!(
            !text.contains(['\u{2013}', '\u{2014}'])
                && !def.description.contains(['\u{2013}', '\u{2014}']),
            "{local}: no em or en dashes"
        );
    }
}

/// The title card's settings bind: the model takes them as the definition says, the plan hands them to the scene, and
/// at the hold its title, subtitle and accent mark show them (a missing one shows its default).
#[test]
fn title_card_params_bind() {
    let id = ContribId::builtin(TITLE_CARD);
    let reg = Registry::builtin();
    assert!(
        reg.scene(&id).is_some(),
        "the built-in registry draws the title card"
    );
    let set = [
        ("title", "Day 1 in Lisbon"),
        ("subtitle", "Walking the old town"),
        ("accent", "#3FB8AF"),
    ];
    let p = project(TITLE_CARD, 4.0, &set);
    klipolo_model::validate(&p).unwrap();
    // The definition holds the settings to its own kinds: an accent that isn't a colour is refused.
    let bad = project(TITLE_CARD, 4.0, &[("accent", "yellow")]);
    assert!(
        klipolo_model::validate(&bad).is_err(),
        "an accent that isn't a colour"
    );
    let long = "x".repeat(200);
    let bad = project(TITLE_CARD, 4.0, &[("title", long.as_str())]);
    assert!(
        klipolo_model::validate(&bad).is_err(),
        "a title past its length"
    );

    let plan = frame_plan(
        &PlanContext::new(Arc::new(p)),
        secs(2.0),
        FormatKind::Landscape,
    );
    assert!(plan.missing.is_empty(), "{:?}", plan.missing);
    let scenes: Vec<_> = plan
        .layers
        .iter()
        .filter_map(|l| match l {
            Layer::Scene(s) => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(scenes.len(), 1, "one scene layer");
    let layer = scenes[0];
    assert_eq!(&*layer.contrib, id.as_str());
    for (key, value) in set {
        assert_eq!(
            layer.params.get(key),
            Some(&ParamValue::Text(value.into())),
            "{key}"
        );
    }

    let inst = SceneInstance::new(&layer.scene, &layer.params);
    let mut f = SceneFrame::for_instance(&inst);
    eval_into(&inst, layer.t, &mut f);
    assert_eq!(text_of(&inst, &f, "title"), "Day 1 in Lisbon");
    assert_eq!(text_of(&inst, &f, "subtitle"), "Walking the old town");
    let teal = [0x3F, 0xB8, 0xAF].map(|c| f32::from(c as u8) / 255.0);
    let mark = f.node("mark");
    for (got, want) in mark.fill.iter().zip(teal) {
        assert!(
            (got - want).abs() < 1e-3,
            "the mark is the accent: {:?}",
            mark.fill
        );
    }
    assert!(f.node("title").drawn && f.node("subtitle").drawn && mark.drawn);

    // Left out, a setting shows its default.
    let defaults = SceneInstance::new(&layer.scene, &BTreeMap::new());
    let mut f = SceneFrame::for_instance(&defaults);
    eval_into(&defaults, layer.t, &mut f);
    assert_eq!(text_of(&defaults, &f, "title"), "Your title");
    let yellow = [0xFF, 0xD0, 0x2E].map(|c| f32::from(c as u8) / 255.0);
    for (got, want) in f.node("mark").fill.iter().zip(yellow) {
        assert!((got - want).abs() < 1e-3, "the brand yellow by default");
    }
}

/// The white ink in the lower third's title area of a frame of `SIZE`.
fn lower_third_title_ink(img: &image::RgbaImage) -> Option<Bounds> {
    let (json, _) = scene_json(LOWER_THIRD);
    ink_bounds_in(img, node_px(&json, "name", (img.width(), img.height())))
}

/// The lower third with its name set, 3 s long: in the preview's registry (the built-in one) its title area shows white
/// ink a good part of its height high, and the export worker's registry for a job has the preset too and draws the
/// same frame. Then a real export (ProRes, the encoder Linux has) of the same project: its frame at 1.5 s shows the
/// name in the same place.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn lower_third_preset_renders_in_preview_and_export() {
    use klipolo_engine::api::ExportQuality;
    use klipolo_engine::export::job::ExportJob;
    use klipolo_engine::export::naming::plan_files;
    use klipolo_engine::export::protocol::Lines;
    use klipolo_engine::export::runner::Batch;
    use klipolo_engine::export::shared::Control;
    use klipolo_engine::export::worker::{Ended, export, fonts_before_frame_zero};

    let id = ContribId::builtin(LOWER_THIRD);
    let preview = Registry::builtin();
    assert!(
        preview.scene(&id).is_some(),
        "the preview's registry draws the lower third"
    );
    let p = project(
        LOWER_THIRD,
        3.0,
        &[("title", "Ana Lee"), ("role", "Product designer")],
    );
    let (json, _) = scene_json(LOWER_THIRD);
    let area = node_px(&json, "name", SIZE);

    let mut h = Headless::new(preview).await.unwrap();
    let p = Arc::new(p);
    let (shown, notes) = scene_frame(&mut h, &p, 1.5, SIZE).await;
    assert!(notes.is_empty(), "{notes:?}");
    keep_still("lower-third", &shown);
    let ink = lower_third_title_ink(&shown).expect("the preview shows the name in the title area");
    assert!(
        ink.height() >= 0.3 * (area.y1 - area.y0) && ink.within(area),
        "the name fills its line: {ink:?} in {area:?}"
    );
    let (blank, _) = scene_frame(&mut h, &p, 0.0, SIZE).await;
    assert!(
        lower_third_title_ink(&blank).is_none(),
        "nothing before it wipes in"
    );

    // The export worker's registry for a job that carries no contributions still has the built-in.
    let job_dir = tempfile::tempdir().unwrap();
    let export_registry = job_registry(job_dir.path());
    assert!(
        export_registry.scene(&id).is_some(),
        "the export worker's registry draws it"
    );
    let mut e = Headless::new(export_registry).await.unwrap();
    let (exported, _) = scene_frame(&mut e, &p, 1.5, SIZE).await;
    let (within, _) = klipolo_engine::test_support::compare(&shown, &exported, 2);
    assert!(
        within >= 0.999,
        "the export's registry draws the preview's frame: {within}"
    );

    // And through a real export.
    ffmpeg::init().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let project = (*p).clone();
    let mut files = plan_files(
        &project,
        &[FormatKind::Landscape],
        ExportQuality::Master,
        dir.path(),
    );
    (files[0].width, files[0].height) = SIZE;
    let batch = Batch {
        project,
        fps: 30,
        quality: ExportQuality::Master,
        files,
        pcm_dir: None,
        speech: Arc::default(),
        text_language: "en".into(),
        contribs: Default::default(),
        caption_file: None,
        allow_stand_ins: false,
        activity_dir: None,
        allow_missing_cursor: false,
    };
    let job = ExportJob::create(&batch, 30).unwrap();
    fonts_before_frame_zero(&job);
    let ended = export(
        job.clone(),
        &mut Lines::new(Vec::new()),
        Arc::new(Control::default()),
    )
    .await;
    assert!(matches!(ended, Ok(Ended::Done)), "{ended:?}");
    let after = ExportJob::open(&job.dir).unwrap();
    let frame = decoded_rgba(&after.dir.join(&after.outputs[0].file), 45);
    keep_still("lower-third-exported", &frame);
    let got = lower_third_title_ink(&frame).expect("the exported frame shows the name");
    for (a, b) in [
        (got.x0, ink.x0),
        (got.y0, ink.y0),
        (got.x1, ink.x1),
        (got.y1, ink.y1),
    ] {
        assert!(
            (a - b).abs() <= 3.0,
            "the exported name is where the preview drew it: {got:?} vs {ink:?}"
        );
    }
}

/// The title card with `title` and `accent` drawn at its hold: the title's white ink, which must be centred in its area,
/// and the colour at the middle of its accent mark.
async fn title_card_at_hold(
    h: &mut Headless,
    title: &str,
    accent: &str,
    still: &str,
) -> (Bounds, [u8; 3]) {
    let (json, _) = scene_json(TITLE_CARD);
    let area = node_px(&json, "title", SIZE);
    let mark = node_px(&json, "mark", SIZE);
    let p = Arc::new(project(
        TITLE_CARD,
        4.0,
        &[
            ("title", title),
            ("subtitle", "Walking the old town"),
            ("accent", accent),
        ],
    ));
    let (img, notes) = scene_frame(h, &p, 2.0, SIZE).await;
    assert!(notes.is_empty(), "{notes:?}");
    keep_still(still, &img);
    let ink = ink_bounds_in(&img, area).unwrap_or_else(|| panic!("{title}: no title ink"));
    let centre = (ink.x0 + ink.x1) / 2.0;
    assert!(
        (centre - f64::from(SIZE.0) / 2.0).abs() <= 4.0 && ink.within(area),
        "{title}: centred in its area: {ink:?} in {area:?}"
    );
    let (cx, cy) = ((mark.x0 + mark.x1) / 2.0, (mark.y0 + mark.y1) / 2.0);
    (ink, rgb_at(&img, cx as u32, cy as u32))
}

/// The title card drawn with its settings: the title's white ink centred in its area, wider for a longer title, and the
/// accent mark in the accent; nothing of it before it starts.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn title_card_draws_its_settings() {
    let mut h = Headless::new(Registry::builtin()).await.unwrap();
    assert!(
        Registry::builtin()
            .scene(&ContribId::builtin(TITLE_CARD))
            .is_some(),
        "the built-in registry draws the title card"
    );
    let (short, yellow) = title_card_at_hold(&mut h, "Day 1", "#FFD02E", "title-card").await;
    let (long, teal) =
        title_card_at_hold(&mut h, "Day 1 in Lisbon", "#3FB8AF", "title-card-teal").await;
    assert!(
        long.x1 - long.x0 > 1.5 * (short.x1 - short.x0),
        "{short:?} vs {long:?}"
    );
    let near = |got: [u8; 3], want: [u8; 3]| got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 6);
    assert!(
        near(yellow, [0xFF, 0xD0, 0x2E]),
        "the brand yellow mark: {yellow:?}"
    );
    assert!(near(teal, [0x3F, 0xB8, 0xAF]), "the accent mark: {teal:?}");
    let (json, _) = scene_json(TITLE_CARD);
    let p = Arc::new(project(TITLE_CARD, 4.0, &[]));
    let (img, _) = scene_frame(&mut h, &p, 0.0, SIZE).await;
    assert!(
        ink_bounds_in(&img, node_px(&json, "title", SIZE)).is_none(),
        "nothing before it starts"
    );
}

/// Each format a video can be made in, its shorter side 1080 pixels: landscape, portrait (9:16) and square.
const FORMATS: [(FormatKind, (u32, u32), &str); 3] = [
    (FormatKind::Landscape, (1920, 1080), "16x9"),
    (FormatKind::Portrait, (1080, 1920), "9x16"),
    (FormatKind::Square, (1080, 1080), "1x1"),
];

/// The kinetic title: nothing before it starts, its letters rising in from below at first, the whole title in its
/// area, centred, once they are up; in 9:16 inside the area the apps keep clear.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn kinetic_title_letters_rise_into_place() {
    let mut h = Headless::new(Registry::builtin()).await.unwrap();
    let (json, _) = scene_json(KINETIC_TITLE);
    let p = Arc::new(project(KINETIC_TITLE, 3.0, &[("title", "We went flying")]));
    let area = node_px(&json, "title", SIZE);
    let (img, _) = scene_frame(&mut h, &p, 0.0, SIZE).await;
    assert!(
        ink_bounds_in(&img, area).is_none(),
        "nothing before it starts"
    );
    // Part way: the last letters are still below their place, or not in yet.
    let (early, notes) = scene_frame(&mut h, &p, 0.3, SIZE).await;
    assert!(notes.is_empty(), "{notes:?}");
    let (img, _) = scene_frame(&mut h, &p, 1.5, SIZE).await;
    keep_still("kinetic-title", &img);
    let up = ink_bounds_in(&img, area).expect("the title, once up");
    assert!(
        ((up.x0 + up.x1) / 2.0 - f64::from(SIZE.0) / 2.0).abs() <= 4.0 && up.within(area),
        "centred in its area: {up:?} in {area:?}"
    );
    let rising = bounds_where(&early, |px| px.iter().all(|&c| c > 200));
    assert!(
        rising.is_none_or(|b| b.x1 < up.x1 || b.y1 > up.y1),
        "rising in: {rising:?} vs {up:?}"
    );
    for (format, size, name) in FORMATS {
        let (img, notes) = scene_frame_in(&mut h, &p, 1.5, size, format).await;
        assert!(notes.is_empty(), "{name}: {notes:?}");
        let drawn = bounds_where(&img, |px| px.iter().all(|&c| c > 200))
            .unwrap_or_else(|| panic!("{name}: no title"));
        if format == FormatKind::Portrait {
            inside_the_clear_area("the kinetic title", drawn, size);
        }
    }
}

/// The box holding `img`'s pixels that `keep` picks, if any.
fn bounds_where(img: &image::RgbaImage, keep: impl Fn([u8; 3]) -> bool) -> Option<Bounds> {
    let mut b: Option<Bounds> = None;
    for (x, y, p) in img.enumerate_pixels() {
        if !keep([p[0], p[1], p[2]]) {
            continue;
        }
        let (x, y) = (f64::from(x), f64::from(y));
        b = Some(match b {
            None => Bounds {
                x0: x,
                y0: y,
                x1: x + 1.0,
                y1: y + 1.0,
            },
            Some(o) => Bounds {
                x0: o.x0.min(x),
                y0: o.y0.min(y),
                x1: o.x1.max(x + 1.0),
                y1: o.y1.max(y + 1.0),
            },
        });
    }
    b
}

/// The templates in every format, as full-frame layers. The lower third's name shows in its box, nothing of it reaches
/// the frame's edge, and in 9:16 all of it stays above the bottom 20 % the platforms cover. The
/// title card's title and accent mark stay centred, and the mark keeps its shape: the same aspect in every format. In
/// 9:16 the ink of both (their words, rules and plates, not the title card's dim scrim) stays inside the area the apps'
/// buttons and captions leave clear (`PORTRAIT_CLEAR`).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn templates_keep_their_shape_in_every_format() {
    let mut h = Headless::new(Registry::builtin()).await.unwrap();
    let (lower, _) = scene_json(LOWER_THIRD);
    let (card, _) = scene_json(TITLE_CARD);
    let named = Arc::new(project(
        LOWER_THIRD,
        3.0,
        &[("title", "Ana Lee"), ("role", "Product designer")],
    ));
    let titled = Arc::new(project(
        TITLE_CARD,
        4.0,
        &[
            ("title", "Day 1 in Lisbon"),
            ("subtitle", "Walking the old town"),
        ],
    ));
    let mut aspects = Vec::new();
    for (format, size, name) in FORMATS {
        let (w, tall) = (f64::from(size.0), f64::from(size.1));
        let (img, notes) = scene_frame_in(&mut h, &named, 1.5, size, format).await;
        assert!(notes.is_empty(), "{name}: {notes:?}");
        keep_still(&format!("lower-third-{name}"), &img);
        let area = node_px(&lower, "name", size);
        let ink = ink_bounds_in(&img, area).unwrap_or_else(|| panic!("{name}: no name"));
        assert!(
            ink.within(area) && ink.height() >= 0.3 * (area.y1 - area.y0),
            "{name}: the name fills its line: {ink:?} in {area:?}"
        );
        let drawn = bounds_where(&img, |p| p.iter().any(|&c| c > 40)).unwrap();
        assert!(
            drawn.x0 > 0.0 && drawn.y0 > 0.0 && drawn.x1 < w && drawn.y1 < tall,
            "{name}: inside the frame: {drawn:?}"
        );
        if format == FormatKind::Portrait {
            assert!(
                drawn.y1 <= 0.80 * tall,
                "9:16: above the bottom 20 %, not down to {:.3} of the height",
                drawn.y1 / tall
            );
            inside_the_clear_area("the lower third", drawn, size);
        }

        let (img, notes) = scene_frame_in(&mut h, &titled, 2.0, size, format).await;
        assert!(notes.is_empty(), "{name}: {notes:?}");
        keep_still(&format!("title-card-{name}"), &img);
        let title = ink_bounds_in(&img, node_px(&card, "title", size))
            .unwrap_or_else(|| panic!("{name}: no title"));
        let mark = bounds_where(&img, |p| {
            p.iter()
                .zip([0xFF, 0xD0, 0x2E])
                .all(|(&a, b)| a.abs_diff(b) <= 40)
        })
        .unwrap_or_else(|| panic!("{name}: no mark"));
        for (what, b) in [("title", title), ("mark", mark)] {
            assert!(
                ((b.x0 + b.x1) / 2.0 - w / 2.0).abs() <= 4.0,
                "{name}: the {what} is centred: {b:?}"
            );
        }
        aspects.push((name, (mark.x1 - mark.x0) / mark.height()));
        if format == FormatKind::Portrait {
            // Its title, the longest in the shrink-to-fit width, with the mark and the subtitle.
            let card = bounds_where(&img, |p| p.iter().any(|&c| c > 40))
                .unwrap_or_else(|| panic!("{name}: no title card"));
            inside_the_clear_area("the title card", card, size);
        }
    }
    let wide = aspects[0].1;
    assert!(
        aspects.iter().all(|(_, a)| (a / wide - 1.0).abs() <= 0.2),
        "the mark keeps its shape in every format: {aspects:?}"
    );
}

/// `ink` (pixels of a frame `size`) is inside 9:16's clear area (`PORTRAIT_CLEAR`), where no app's buttons, tabs or
/// captions cover it.
fn inside_the_clear_area(what: &str, ink: Bounds, size: (u32, u32)) {
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let c = PORTRAIT_CLEAR;
    let clear = Bounds {
        x0: c.x * w,
        y0: c.y * h,
        x1: (c.x + c.w) * w,
        y1: (c.y + c.h) * h,
    };
    assert!(
        ink.within(clear),
        "9:16: {what} stays inside the clear area {clear:?}, not {ink:?}"
    );
}

/// Left empty, the role and the subtitle show nothing: the lower third is its name alone and the title card its title
/// alone, the project takes the blanks, and no frame notes a problem.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_empty_role_or_subtitle_leaves_its_line_out() {
    let mut h = Headless::new(Registry::builtin()).await.unwrap();
    for (local, at, filled, blank) in [
        (LOWER_THIRD, 1.5, ("title", "name"), ("role", "role")),
        (
            TITLE_CARD,
            2.0,
            ("title", "title"),
            ("subtitle", "subtitle"),
        ),
    ] {
        let (json, _) = scene_json(local);
        let p = project(local, 4.0, &[(filled.0, "Ana Lee"), (blank.0, "")]);
        klipolo_model::validate(&p).unwrap();
        let (img, notes) = scene_frame(&mut h, &Arc::new(p), at, SIZE).await;
        assert!(notes.is_empty(), "{local}: {notes:?}");
        keep_still(&format!("{local}-blank-{}", blank.0), &img);
        assert!(
            ink_bounds_in(&img, node_px(&json, filled.1, SIZE)).is_some(),
            "{local}: its {} shows",
            filled.0
        );
        assert!(
            ink_bounds_in(&img, node_px(&json, blank.1, SIZE)).is_none(),
            "{local}: an empty {} shows nothing",
            blank.0
        );
    }
}

/// Frame `at` of `path` decoded to RGBA (BT.601 here, BT.709 in the file: white stays white).
fn decoded_rgba(path: &Path, at: usize) -> image::RgbaImage {
    use ffmpeg::format::Pixel;
    use ffmpeg::software::scaling::{Context as Scaler, Flags};
    let mut input = ffmpeg::format::input(path).unwrap();
    let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
    let index = stream.index();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    let (mut frame, mut n, mut out) = (ffmpeg::frame::Video::empty(), 0usize, None);
    let mut take = |decoder: &mut ffmpeg::decoder::Video, out: &mut Option<image::RgbaImage>| {
        while decoder.receive_frame(&mut frame).is_ok() {
            if n == at {
                let (w, h) = (frame.width(), frame.height());
                let mut scaler =
                    Scaler::get(frame.format(), w, h, Pixel::RGBA, w, h, Flags::BILINEAR).unwrap();
                let mut rgba = ffmpeg::frame::Video::empty();
                scaler.run(&frame, &mut rgba).unwrap();
                let stride = rgba.stride(0);
                let bytes: Vec<u8> = (0..h as usize)
                    .flat_map(|y| rgba.data(0)[y * stride..y * stride + w as usize * 4].to_vec())
                    .collect();
                *out = image::RgbaImage::from_raw(w, h, bytes);
            }
            n += 1;
        }
    };
    for (s, packet) in input.packets() {
        if s.index() == index {
            decoder.send_packet(&packet).unwrap();
            take(&mut decoder, &mut out);
        }
    }
    decoder.send_eof().unwrap();
    take(&mut decoder, &mut out);
    out.unwrap_or_else(|| panic!("{} has no frame {at}", path.display()))
}

/// Animated words (`media::text_motion`), a scene their settings make, drawn through a generator layer in its own
/// box: nothing of a word shown before its turn (only the first word is in part way), every word in the box once
/// settled, and white as their settings say.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn animated_words_come_in_word_by_word_in_their_box() {
    use klipolo_model::media::text_motion::{self, From, TextMotion, Unit};
    use klipolo_model::timeline::TextStyle;
    let mut h = Headless::new(Registry::builtin()).await.unwrap();
    let motion = TextMotion {
        unit: Unit::Word,
        from: From {
            y: Some(80.0),
            opacity: Some(0.0),
            ..From::default()
        },
        unit_secs: Some(0.3),
        ease: None,
        stagger_secs: Some(0.6),
        order: None,
        exit: false,
    };
    let style = TextStyle {
        shadow: false,
        ..TextStyle::default()
    };
    let params = text_motion::params("ONE TWO THREE", &style, &motion);
    let content = ItemContent::generator(text_motion::source(), params);
    let (x, y, w, hh) = (0.1, 0.3, 0.8, 0.2);
    let mut p = Project::new("Words");
    p.timeline.tracks.push(overlay_track(
        "words",
        vec![item("words", 0.0, 4.0, bx(x, y, w, hh), content)],
    ));
    let p = Arc::new(p);
    let (sw, shh) = (f64::from(SIZE.0), f64::from(SIZE.1));
    let area = Bounds {
        x0: x * sw - 2.0,
        y0: y * shh - 2.0,
        x1: (x + w) * sw + 2.0,
        y1: (y + hh) * shh + 2.0,
    };
    let white = |px: [u8; 3]| px.iter().all(|&c| c > 200);
    let (settled, notes) = scene_frame(&mut h, &p, 3.0, SIZE).await;
    assert!(notes.is_empty(), "{notes:?}");
    keep_still("animated-words", &settled);
    let all = bounds_where(&settled, white).expect("the words, once in");
    assert!(all.within(area), "inside their box: {all:?} in {area:?}");
    // Part way through the first word's turn, only the first word is in: what shows ends well short of all three.
    let (early, _) = scene_frame(&mut h, &p, 0.5, SIZE).await;
    let first = bounds_where(&early, white).expect("the first word");
    assert!(
        first.x1 - first.x0 < (all.x1 - all.x0) * 0.6,
        "one word, not all: {first:?} vs {all:?}"
    );
}
