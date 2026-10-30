//! Scenes in the render worker (lavapipe in CI, `FakeMeter`): `CheckScene` answers a cost with the scene's CPU time and
//! CPU time and a thumbnail; a submitted `svg` node is stored as paths the main process's loader accepts; a fill names
//! a library generator only when `contribs.json` hands it over; and frames of a project placing the draft the
//! it over the still, while a draft it never checked draws nothing, noted as being checked.

mod support;
use klipolo_engine::make::frames::ContribEntry;
use klipolo_engine::make::measure::FakeMeter;
use klipolo_engine::make::protocol::WorkerReply;
use klipolo_model::contrib::{ContribDef, ContribKind};
use klipolo_model::layout::BoxN;
use klipolo_model::project::{AssetKind, Project};
use klipolo_model::scene::Scene;
use klipolo_model::timeline::{Clip, ClipSource, ItemContent};
use support::*;

/// A plain SVG: a blue square and an orange circle.
const LOGO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <rect x="5" y="5" width="50" height="50" fill="#2D6CDF"/>
  <circle cx="70" cy="70" r="25" fill="#FF7A00"/></svg>"##;
/// The library generator a fill names.
const LIB_WHITE: &str = "lib.k3f9q2ma/generator.white.v1";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn check_scene_answers_a_cost_a_thumbnail_and_cpu_time() {
    let _gpu = gpu_turn().await;
    let mut w = InProcessWorker::new(FakeMeter::new(0.2)).await;
    let r = w.check_scene(LOWER_THIRD, &[], &[]).await;
    assert!(r.ok, "{:?}", r.diagnostics);
    let cost = r.cost.as_ref().expect("a cost");
    assert!(cost.preview_ms.p95 > 0.0, "{cost:?}");
    assert!(
        cost.cpu_us.is_some_and(|c| c.p95 > 0.0),
        "CPU time: {cost:?}"
    );
    assert!(r.key.is_some() && r.thumbnail().is_some());
    let stored = r.stored();
    assert_eq!(
        klipolo_engine::make::scene_check::normalise(stored.as_bytes()).unwrap(),
        stored.as_bytes(),
        "the stored bytes are normalised"
    );
    Scene::from_stored(stored.as_bytes()).expect("the main process's loader takes them");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_svg_node_is_stored_as_paths() {
    let _gpu = gpu_turn().await;
    let mut w = InProcessWorker::new(FakeMeter::new(0.2)).await;
    let json = r##"{"scene":1,"duration":2,"nodes":[
      {"type":"rect","box":[0,0,1,1],"fill":"#101018"},
      {"id":"logo","type":"svg","src":"logo","box":[0.3,0.2,0.4,0.6]}],
      "timeline":[{"from":"#logo","at":0,"dur":0.5,"opacity":0}]}"##;
    let r = w.check_scene(json, &[("logo", LOGO)], &[]).await;
    assert!(r.ok, "{:?}", r.diagnostics);
    let stored = r.stored();
    assert!(
        stored.contains(r#""type":"path""#)
            && !stored.contains(r#""type":"svg""#)
            && !stored.contains("src"),
        "{stored}"
    );
    Scene::from_stored(stored.as_bytes()).expect("the loader takes what was stored");
    // An svg node naming an SVG the request didn't bring is named by its path.
    let r = w.check_scene(json, &[("other", LOGO)], &[]).await;
    assert!(!r.ok);
    assert_eq!(r.diagnostics[0].pointer.as_deref(), Some("/nodes/1/src"));
    assert_eq!(r.diagnostics[0].code, "scene.svgMissing");
}

/// The library generator `LIB_WHITE`, as the app hands it over.
fn white() -> ContribEntry {
    ContribEntry {
        def: ContribDef::new(LIB_WHITE, ContribKind::Generator, "White"),
        body: "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }".into(),
        runs_at: Default::default(),
        envelope: Default::default(),
        draft: false,
    }
}

/// A rect filled with `LIB_WHITE`.
fn filled_white() -> String {
    format!(
        r##"{{"scene":1,"duration":2,"nodes":[
          {{"type":"rect","box":[0.1,0.1,0.5,0.5],"radius":20,"fill":{{"generator":"{LIB_WHITE}","params":{{}}}}}}]}}"##
    )
}

/// Frames of a stored scene alone draw its library fill once the generator is handed over (as its
/// check had it); without it they are refused, said as the scene's problem at its place, never as a shader's.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn alone_frames_take_the_generators_a_scenes_fills_name() {
    let _gpu = gpu_turn().await;
    let mut w = InProcessWorker::new(FakeMeter::new(0.2)).await;
    let r = w.check_scene(&filled_white(), &[], &[white()]).await;
    assert!(r.ok, "{:?}", r.diagnostics);
    let stored = r.stored();
    let replies = |lines: Vec<String>| -> Vec<WorkerReply> {
        lines
            .iter()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    };
    let drawn = replies(
        w.frames_scene_alone_lines(&stored, Some(&[white()]), &[0.5])
            .await,
    );
    assert!(
        drawn.iter().any(|r| matches!(r, WorkerReply::Frame { .. })),
        "{drawn:?}"
    );
    let refused = replies(w.frames_scene_alone_lines(&stored, None, &[0.5]).await);
    assert!(
        refused
            .iter()
            .any(|r| matches!(r, WorkerReply::Failed { reason, .. }
            if reason.contains("the scene was refused at /nodes/0/fill/generator"))),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_fill_names_a_library_generator_only_when_it_is_handed_over() {
    let _gpu = gpu_turn().await;
    let mut w = InProcessWorker::new(FakeMeter::new(0.2)).await;
    let json = filled_white();
    let r = w.check_scene(&json, &[], &[white()]).await;
    assert!(r.ok, "{:?}", r.diagnostics);
    let r = w.check_scene(&json, &[], &[]).await;
    assert!(!r.ok);
    let d = &r.diagnostics[0];
    assert_eq!(
        (d.code, d.pointer.as_deref()),
        ("scene.unknownGenerator", Some("/nodes/0/fill/generator"))
    );
}

/// A one-video project (its still a light grey) with layer `lt` placing `source` over the whole frame for 4 s.
fn placing(source: &str) -> Project {
    let mut p = Project::new("Placed");
    p.assets = vec![klipolo_model::project::Asset {
        id: "v".into(),
        kind: AssetKind::Video,
        path: "v.mp4".into(),
        name: "v".into(),
        duration_us: secs(60.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: Some(0),
        has_audio: false,
        audio_lead_us: None,
    }];
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "v".into() },
        0,
        secs(4.0),
    )];
    let content = ItemContent::generator(source.into(), Default::default());
    let whole = BoxN {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };
    p.timeline.tracks = vec![overlay_track(
        "titles",
        vec![item("lt", 0.0, 4.0, whole, content)],
    )];
    p
}

/// The mean RGB of the 5 × 5 patch around `(x, y)` (fractions of the frame).
fn patch(img: &image::RgbImage, (x, y): (f64, f64)) -> [f64; 3] {
    let (cx, cy) = (
        (x * f64::from(img.width())) as u32,
        (y * f64::from(img.height())) as u32,
    );
    let mut sum = [0.0; 3];
    for dy in 0..5 {
        for dx in 0..5 {
            let p = img.get_pixel(cx + dx - 2, cy + dy - 2).0;
            for k in 0..3 {
                sum[k] += f64::from(p[k]) / 25.0;
            }
        }
    }
    sum
}

/// Frames of a project placing the draft the worker just checked draw it over the still: the lower third's plate
/// shows dark where the bare still is light, as the draft's own thumbnail shows it (its plate made opaque here, so the
/// match whatever is under them: within 8/255), with no "checking" note. A draft whose
/// bytes it never checked draws nothing and says so.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn project_frames_draw_a_placed_scene_draft_over_the_still() {
    let _gpu = gpu_turn().await;
    let mut w = InProcessWorker::new(FakeMeter::new(0.2)).await;
    let opaque = LOWER_THIRD.replace("rgba(20, 20, 28, 0.85)", "#14141C");
    assert_ne!(opaque, LOWER_THIRD, "its plate made opaque");
    let r = w.check_scene(&opaque, &[], &[]).await;
    assert!(r.ok, "{:?}", r.diagnostics);
    let stored = r.stored();
    let scene = Scene::from_stored(stored.as_bytes()).unwrap();
    let id = "draft.local/scene.d-test";
    let def = ContribDef {
        params: scene.params.clone(),
        ..ContribDef::new(id, ContribKind::Scene, "Lower third")
    };
    let entry = ContribEntry {
        def,
        body: stored.clone(),
        runs_at: Default::default(),
        envelope: r.cost.as_ref().unwrap().envelope.clone(),
        draft: true,
    };
    let still = image::RgbaImage::from_pixel(640, 360, image::Rgba([210, 210, 210, 255]));
    let (bare, _) = w
        .frames_project_with(placing(id), &[], &[2.0], still.clone())
        .await;
    let (placed, notes) = w
        .frames_project_with(
            placing(id),
            std::slice::from_ref(&entry),
            &[2.0],
            still.clone(),
        )
        .await;
    assert!(
        !notes
            .iter()
            .any(|n| n.starts_with("frame.agentEffectChecking")),
        "{notes:?}"
    );
    let decode = |f: &FrameView| image::load_from_memory(&f.jpeg).unwrap().to_rgb8();
    // Inside the plate, right of the role's box and below the title's.
    let at = (0.04 + 0.6 * 0.8, 0.62 + 0.26 * 0.75);
    let (b, p) = (patch(&decode(&bare[0]), at), patch(&decode(&placed[0]), at));
    assert!(b[0] > 180.0, "the bare still: {b:?}");
    assert!(p.iter().all(|c| *c < 90.0), "the plate over it: {p:?}");
    let thumb = patch(&r.thumbnail().unwrap(), at);
    assert!(
        thumb.iter().all(|c| *c < 90.0),
        "the thumbnail's plate: {thumb:?}"
    );
    assert!(
        (0..3).all(|k| (p[k] - thumb[k]).abs() <= 8.0),
        "the frame's plate {p:?} as the thumbnail's {thumb:?}"
    );
    // Bytes this worker never checked: nothing drawn, noted as being checked.
    let other = ContribEntry {
        body: stored.replace("Ana Lee", "Someone else"),
        ..entry
    };
    let (unchecked, notes) = w
        .frames_project_with(placing(id), &[other], &[2.0], still)
        .await;
    assert!(
        notes
            .iter()
            .any(|n| n.starts_with("frame.agentEffectChecking")),
        "{notes:?}"
    );
    let u = patch(&decode(&unchecked[0]), at);
    assert!(u[0] > 180.0, "drawn as nothing: {u:?}");
}

/// `Verify` of a library scene's folder (the bytes the app loaded) is checked and measured as a submitted scene
/// is, and answers the folder's own key, so the app stores its cost and the snapshot then holds it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn verify_of_a_scene_folder_answers_its_key_with_a_cost() {
    use klipolo_engine::make::protocol::{Key, WorkerReply};
    use klipolo_engine::registry::key::{ContribSource, contrib_key};
    use klipolo_engine::registry::pack::hex;
    use sha2::{Digest, Sha256};
    let _gpu = gpu_turn().await;
    let mut w = InProcessWorker::new(FakeMeter::new(0.2)).await;
    let bytes = klipolo_engine::make::scene_check::normalise(LOWER_THIRD.as_bytes()).unwrap();
    let scene = Scene::from_stored(&bytes).unwrap();
    let def = ContribDef {
        params: scene.params.clone(),
        ..ContribDef::new(
            "lib.k3f9q2ma/scene.lower-third.v1",
            ContribKind::Scene,
            "Lower third",
        )
    };
    let mut files = std::collections::BTreeMap::new();
    files.insert(
        "contrib.json".to_string(),
        serde_json::to_vec(&def).unwrap(),
    );
    files.insert("scene.json".to_string(), bytes.clone());
    let hashes: std::collections::BTreeMap<&String, String> = files
        .iter()
        .map(|(n, b)| (n, hex(&Sha256::digest(b))))
        .collect();
    let hashes = serde_json::to_vec(&hashes).unwrap();
    files.insert("hashes.json".to_string(), hashes);
    let files: Vec<(String, Vec<u8>)> = files.into_iter().collect();
    let lines = w.verify(&def.id, &files).await;
    let want = Key(contrib_key(&def, &ContribSource::Scene(bytes)));
    let compiled = lines
        .iter()
        .find_map(|l| match serde_json::from_str(l).ok()? {
            WorkerReply::Compiled { ok, key, cost, .. } => Some((ok, key, cost)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{lines:?}"));
    assert_eq!((compiled.0, compiled.1), (true, Some(want)), "{lines:?}");
    assert!(compiled.2.is_some_and(|c| c.cpu_us.is_some()));
}
