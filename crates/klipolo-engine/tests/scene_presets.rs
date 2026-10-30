//! The shipped motion graphic presets: the lower third (`klipolo.builtin/scene.lower-third`: name, role and accent) and
//! the centred title card (`klipolo.builtin/scene.title-card`: title, subtitle and accent), data folders under
//! `klipolo-model/presets/` in exactly the shape a library version has (`contrib.json` beside `scene.json`). Built-ins,
//! so the preview's registry and the export worker's both draw them; each fills the frame.
//! - each is a scene an agent could have made: its definition's settings are the scene's own, and it checks;
//! - the title card's settings bind: the project keeps them, the plan hands them over, and its nodes show them;
//! - drawn (lavapipe in CI): the lower third's title area shows its name in the preview and in an exported file, and
//!   the title card its title and accent. `PRESET_STILLS=<folder>` keeps a still of each.

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
use klipolo_model::params::{ParamKind, ParamValue};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::scene::Scene;
use klipolo_model::scene::eval::{SceneFrame, eval_into};
use klipolo_model::scene::instance::SceneInstance;
use klipolo_model::timeline::ItemContent;
use support::*;

const LOWER_THIRD: &str = "scene.lower-third";
const TITLE_CARD: &str = "scene.title-card";
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

/// A project of one full-frame layer `layer` showing preset `local` from 0 for `seconds`, with `params`.
fn project(local: &str, seconds: f64, params: &[(&str, &str)]) -> Project {
    let mut p = Project::new("Presets");
    let values: BTreeMap<String, ParamValue> = params
        .iter()
        .map(|(k, v)| (k.to_string(), ParamValue::Text(v.to_string())))
        .collect();
    let content = ItemContent::generator(ContribId::builtin(local), values);
    p.timeline.tracks.push(overlay_track(
        "titles",
        vec![item("layer", 0.0, seconds, bx(0.0, 0.0, 1.0, 1.0), content)],
    ));
    p
}

/// The box (x, y, w, h in fractions of the frame) of node `id` in `scene.json`, inside each group above it.
fn node_box(json: &str, id: &str) -> [f64; 4] {
    fn find(nodes: &[serde_json::Value], id: &str, outer: [f64; 4]) -> Option<[f64; 4]> {
        for n in nodes {
            let b: Vec<f64> = n["box"]
                .as_array()
                .map(|a| a.iter().map(|v| v.as_f64().unwrap()).collect())
                .unwrap_or(vec![0.0, 0.0, 1.0, 1.0]);
            let here = [
                outer[0] + b[0] * outer[2],
                outer[1] + b[1] * outer[3],
                b[2] * outer[2],
                b[3] * outer[3],
            ];
            if n["id"] == id {
                return Some(here);
            }
            if let Some(children) = n["children"].as_array()
                && let Some(found) = find(children, id, here)
            {
                return Some(found);
            }
        }
        None
    }
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    find(v["nodes"].as_array().unwrap(), id, [0.0, 0.0, 1.0, 1.0])
        .unwrap_or_else(|| panic!("no node {id}"))
}

/// `b` in a frame of `SIZE`'s pixels.
fn in_pixels(b: [f64; 4]) -> Bounds {
    let (w, h) = (f64::from(SIZE.0), f64::from(SIZE.1));
    Bounds {
        x0: b[0] * w,
        y0: b[1] * h,
        x1: (b[0] + b[2]) * w,
        y1: (b[1] + b[3]) * h,
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
        (LOWER_THIRD, ["title", "role", "accent"]),
        (TITLE_CARD, ["title", "subtitle", "accent"]),
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
    ink_bounds_in(img, in_pixels(node_box(&json, "name")))
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
    let area = in_pixels(node_box(&json, "name"));

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
    let area = in_pixels(node_box(&json, "title"));
    let mark = in_pixels(node_box(&json, "mark"));
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
        ink_bounds_in(&img, in_pixels(node_box(&json, "title"))).is_none(),
        "nothing before it starts"
    );
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
