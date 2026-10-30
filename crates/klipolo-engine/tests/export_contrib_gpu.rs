//! An export job's contributions: written into the job from memory with their
//! verdicts, and the export worker's registry (`job_registry`) draws exactly those: a checked one from the first frame,
//! nothing that has no verdict there, a generator at half size where its verdict runs it there, and a left-out one
//! noted as being checked (which never stops an export). Drawn for real (lavapipe in CI).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use klipolo_engine::api::is_agent_effect_checking;
use klipolo_engine::export::contrib::{
    CONTRIB_DIR, JobContribs, VERIFIED_JSON, job_registry, write_job_contribs,
};
use klipolo_engine::headless::Headless;
use klipolo_engine::make::budget::{Cost, CostClass, DecidedBy, ExportMs, Pcts, RunsAt};
use klipolo_engine::plan::{Layer, PlanContext, frame_plan};
use klipolo_engine::registry::pack::{LoadedContrib, Verdict, hex, load_files};
use klipolo_model::contrib::{ContribDef, ContribId, ContribKind, Resolution};
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::timeline::{Item, ItemContent, Track, TrackKind};
use sha2::{Digest, Sha256};

const RED: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0, 0.0, 0.0, 1.0); }";

/// A generator version as loaded, `resolution` its definition's.
fn loaded(local: &str, resolution: Option<Resolution>) -> LoadedContrib {
    let id = ContribId::new("lib.k3f9q2ma", local);
    let def = ContribDef {
        resolution,
        ..ContribDef::new(id.as_str(), ContribKind::Generator, "Red")
    };
    let mut files = BTreeMap::new();
    files.insert(
        "contrib.json".to_string(),
        serde_json::to_vec(&def).unwrap(),
    );
    files.insert("effect.wgsl".to_string(), RED.as_bytes().to_vec());
    let hashes: BTreeMap<&String, String> = files
        .iter()
        .map(|(n, b)| (n, hex(&Sha256::digest(b))))
        .collect();
    let hashes = serde_json::to_vec(&hashes).unwrap();
    files.insert("hashes.json".to_string(), hashes);
    load_files(&id, files, PathBuf::new()).unwrap()
}

fn verdict(runs_at: RunsAt) -> Verdict {
    Verdict::of(Cost {
        preview_ms: Pcts::flat(0.3),
        export_ms: ExportMs {
            p1080: 0.6,
            p2160: 2.4,
        },
        cpu_us: None,
        gpu_bytes: Default::default(),
        class: CostClass::Light,
        decided_ms: 0.3,
        at_half: false,
        decided_by: DecidedBy::Time,
        budget_ms: 0.5,
        runs_at,
        resolved: None,
        envelope: Default::default(),
        machine: "vulkan|llvmpipe|test".into(),
        advice: Vec::new(),
        cut: false,
        fenced: None,
    })
    .unwrap()
}

/// A project of one full-frame layer of `id`, 2 s long.
fn project(id: &ContribId) -> Arc<Project> {
    let mut p = Project::new("Export");
    let mut track = Track::new(TrackKind::Overlay, "Layers");
    track.items.push(Item {
        id: "gen".into(),
        start_us: 0,
        duration_us: 2_000_000,
        attach: None,
        layout: Layout::new(
            Anchor::TopLeft,
            BoxN {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
        ),
        content: ItemContent::generator(id.clone(), BTreeMap::new()),
        effects: Vec::new(),
    });
    p.timeline.tracks.push(track);
    Arc::new(p)
}

/// The centre pixel of `p`'s first frame drawn with the job's registry.
async fn centre(h: &mut Headless, p: Arc<Project>) -> [u8; 3] {
    let frame = h
        .render(p, 0, FormatKind::Landscape, (64, 36))
        .await
        .unwrap();
    let i = 18 * frame.stride as usize + 32 * 4;
    [frame.bytes[i], frame.bytes[i + 1], frame.bytes[i + 2]]
}

fn red(px: [u8; 3]) -> bool {
    px[0] > 200 && px[1] < 60 && px[2] < 60
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_job_draws_exactly_its_verified_contributions() {
    let dir = tempfile::tempdir().unwrap();
    let job = dir.path();
    let c = loaded("generator.red.v1", None);
    let contribs = JobContribs {
        runnable: vec![(c.clone(), verdict(RunsAt::default()))],
        withheld: Vec::new(),
    };
    write_job_contribs(job, &contribs).unwrap();
    let folder = job.join(CONTRIB_DIR).join(hex(&c.key));
    assert!(folder.join("effect.wgsl").is_file() && folder.join("contrib.json").is_file());
    let verified: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&std::fs::read(job.join(CONTRIB_DIR).join(VERIFIED_JSON)).unwrap())
            .unwrap();
    assert_eq!(verified.keys().cloned().collect::<Vec<_>>(), [hex(&c.key)]);
    let registry = job_registry(job);
    assert!(registry.shader(&c.def.id).is_some());
    let mut h = Headless::new(registry).await.unwrap();
    assert!(
        red(centre(&mut h, project(&c.def.id)).await),
        "drawn from the first frame"
    );
    // Without its verdict the job draws the layer without it.
    std::fs::write(job.join(CONTRIB_DIR).join(VERIFIED_JSON), b"{}").unwrap();
    let registry = job_registry(job);
    assert!(registry.shader(&c.def.id).is_none());
    h.set_registry(registry);
    assert!(
        !red(centre(&mut h, project(&c.def.id)).await),
        "nothing it has no verdict for"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_generator_its_verdict_halves_renders_at_half_size_in_the_export() {
    let dir = tempfile::tempdir().unwrap();
    let job = dir.path();
    let c = loaded("generator.half.v1", Some(Resolution::Auto));
    let halved = RunsAt {
        half: true,
        ..RunsAt::default()
    };
    let contribs = JobContribs {
        runnable: vec![(c.clone(), verdict(halved))],
        withheld: Vec::new(),
    };
    write_job_contribs(job, &contribs).unwrap();
    let registry = job_registry(job);
    assert_eq!(
        registry.shader(&c.def.id).unwrap().resolution,
        Resolution::Half
    );
    let p = project(&c.def.id);
    let plan = frame_plan(
        &PlanContext::with_registry(p.clone(), registry.clone()),
        0,
        FormatKind::Landscape,
    );
    let half = plan
        .layers
        .iter()
        .any(|l| matches!(l, Layer::Generator(g) if g.half));
    assert!(half, "at half size, as the preview draws it");
    let mut h = Headless::new(registry).await.unwrap();
    assert!(red(centre(&mut h, p).await));
}

/// What the preview's snapshot left out is catalogued in the job, so its frames note it as being checked (which
/// never stops an export) rather than as a missing plugin, and draw without it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_left_out_contribution_is_noted_and_never_stops_the_export() {
    let dir = tempfile::tempdir().unwrap();
    let job = dir.path();
    let c = loaded("generator.out.v1", None);
    let contribs = JobContribs {
        runnable: Vec::new(),
        withheld: vec![c.def.clone()],
    };
    write_job_contribs(job, &contribs).unwrap();
    assert!(
        !job.join(CONTRIB_DIR).join(hex(&c.key)).exists(),
        "no code in the job"
    );
    let registry = job_registry(job);
    assert!(registry.catalog().get(&c.def.id).is_some() && registry.shader(&c.def.id).is_none());
    let p = project(&c.def.id);
    let plan = frame_plan(
        &PlanContext::with_registry(p.clone(), registry.clone()),
        0,
        FormatKind::Landscape,
    );
    assert!(plan.missing.iter().all(|m| m.checking) && !plan.missing.is_empty());
    let note = klipolo_engine::api::agent_effect_checking(c.def.id.as_str());
    assert!(is_agent_effect_checking(&note));
    let mut h = Headless::new(registry).await.unwrap();
    assert!(!red(centre(&mut h, p).await));
}

/// A full-frame red rect that holds still.
const RED_SCENE: &str = r##"{"scene":1,"duration":2,"nodes":[{"type":"rect","id":"r","box":[0,0,1,1],"fill":"#FF0000"}]}"##;

/// A library scene version as loaded.
fn loaded_scene(local: &str, json: &str) -> LoadedContrib {
    let id = ContribId::new("lib.k3f9q2ma", local);
    let def = ContribDef::new(id.as_str(), ContribKind::Scene, "Red card");
    let mut files = BTreeMap::new();
    files.insert(
        "contrib.json".to_string(),
        serde_json::to_vec(&def).unwrap(),
    );
    files.insert("scene.json".to_string(), json.as_bytes().to_vec());
    let hashes: BTreeMap<&String, String> = files
        .iter()
        .map(|(n, b)| (n, hex(&Sha256::digest(b))))
        .collect();
    let hashes = serde_json::to_vec(&hashes).unwrap();
    files.insert("hashes.json".to_string(), hashes);
    load_files(&id, files, PathBuf::new()).unwrap()
}

/// The job carries a verified library scene (its bytes from memory, its verdict), and the export
/// worker compiles it from the job's folder (`compile_verified`) and draws it from the first frame. With the switch off
/// the snapshot hands it over withheld: the job notes it as being checked and draws without it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_verified_library_scene_draws_in_the_export_and_not_with_the_switch_off() {
    let dir = tempfile::tempdir().unwrap();
    let job = dir.path();
    let c = loaded_scene("scene.red.v1", RED_SCENE);
    let contribs = JobContribs {
        runnable: vec![(c.clone(), verdict(RunsAt::default()))],
        withheld: Vec::new(),
    };
    write_job_contribs(job, &contribs).unwrap();
    let folder = job.join(CONTRIB_DIR).join(hex(&c.key));
    assert!(folder.join("scene.json").is_file());
    let registry = job_registry(job);
    assert!(
        registry.scene(&c.def.id).is_some(),
        "compiled from the job's folder"
    );
    let mut h = Headless::new(registry).await.unwrap();
    assert!(
        red(centre(&mut h, project(&c.def.id)).await),
        "drawn from the first frame"
    );
    let off = tempfile::tempdir().unwrap();
    let contribs = JobContribs {
        runnable: Vec::new(),
        withheld: vec![c.def.clone()],
    };
    write_job_contribs(off.path(), &contribs).unwrap();
    let registry = job_registry(off.path());
    assert!(registry.catalog().get(&c.def.id).is_some() && registry.scene(&c.def.id).is_none());
    let plan = frame_plan(
        &PlanContext::with_registry(project(&c.def.id), registry.clone()),
        0,
        FormatKind::Landscape,
    );
    assert!(!plan.missing.is_empty() && plan.missing.iter().all(|m| m.checking));
    h.set_registry(registry);
    assert!(
        !red(centre(&mut h, project(&c.def.id)).await),
        "the switch off: drawn without it"
    );
}

/// How many video frames `path` holds, its container's length (µs) and the mean RGB of frame `at` (decoded with FFmpeg
/// and converted to RGB24).
fn decoded(path: &std::path::Path, at: usize) -> (usize, i64, [f64; 3]) {
    use ffmpeg::format::Pixel;
    use ffmpeg::software::scaling::{Context as Scaler, Flags};
    let mut input = ffmpeg::format::input(path).unwrap();
    let length = input.duration();
    let stream = input.streams().best(ffmpeg::media::Type::Video).unwrap();
    let index = stream.index();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .unwrap()
        .decoder()
        .video()
        .unwrap();
    let (mut frame, mut n, mut mean) = (ffmpeg::frame::Video::empty(), 0usize, [0.0; 3]);
    let mut take = |decoder: &mut ffmpeg::decoder::Video| {
        while decoder.receive_frame(&mut frame).is_ok() {
            if n == at {
                let (w, h) = (frame.width(), frame.height());
                let mut rgb = ffmpeg::frame::Video::empty();
                Scaler::get(frame.format(), w, h, Pixel::RGB24, w, h, Flags::BILINEAR)
                    .unwrap()
                    .run(&frame, &mut rgb)
                    .unwrap();
                let (stride, data) = (rgb.stride(0), rgb.data(0));
                let mut sum = [0u64; 3];
                for y in 0..h as usize {
                    for x in 0..w as usize {
                        for (c, s) in sum.iter_mut().enumerate() {
                            *s += u64::from(data[y * stride + x * 3 + c]);
                        }
                    }
                }
                let count = f64::from(w * h);
                mean = sum.map(|s| s as f64 / count);
            }
            n += 1;
        }
    };
    for (s, packet) in input.packets() {
        if s.index() == index {
            decoder.send_packet(&packet).unwrap();
            take(&mut decoder);
        }
    }
    decoder.send_eof().unwrap();
    take(&mut decoder);
    (n, length, mean)
}

/// A project's length, end to end: an empty project with a Waves layer and a text, 0–10 s each, no
/// main clips, no media files and no sound sources, exports through a real job and the worker's own `export` (ProRes,
/// the encoder Linux has): it finishes, its file is 10 s of frames, and the frame at 5 s isn't black.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_canvas_of_only_layers_exports_ten_seconds() {
    use klipolo_engine::api::ExportQuality;
    use klipolo_engine::export::job::ExportJob;
    use klipolo_engine::export::naming::plan_files;
    use klipolo_engine::export::protocol::Lines;
    use klipolo_engine::export::runner::Batch;
    use klipolo_engine::export::shared::Control;
    use klipolo_engine::export::worker::{Ended, export, fonts_before_frame_zero};
    use klipolo_model::timeline::TextStyle;
    ffmpeg::init().unwrap();
    let mut p = Project::new("Canvas");
    let full = Layout::new(
        Anchor::TopLeft,
        BoxN {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
    );
    let layer = |id: &str, layout: Layout, content: ItemContent| Item {
        id: id.into(),
        start_us: 0,
        duration_us: 10_000_000,
        attach: None,
        layout,
        content,
        effects: Vec::new(),
    };
    let mut background = Track::new(TrackKind::Overlay, "Background");
    background.items.push(layer(
        "bg",
        full,
        ItemContent::generator("klipolo.builtin/generator.waves".into(), BTreeMap::new()),
    ));
    let mut titles = Track::new(TrackKind::Overlay, "Titles");
    titles.items.push(layer(
        "title",
        Layout::new(
            Anchor::TopLeft,
            BoxN {
                x: 0.1,
                y: 0.4,
                w: 0.8,
                h: 0.2,
            },
        ),
        ItemContent::Text {
            text: "Hello".into(),
            style: TextStyle::default(),
        },
    ));
    p.timeline.tracks = vec![background, titles];
    assert!(p.timeline.main.is_empty() && p.assets.is_empty());

    let dir = tempfile::tempdir().unwrap();
    let mut files = plan_files(
        &p,
        &[FormatKind::Landscape],
        ExportQuality::Master,
        dir.path(),
    );
    // A small frame keeps lavapipe and the software encoder quick; the length is what this pins.
    (files[0].width, files[0].height) = (320, 180);
    let batch = Batch {
        project: p,
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
    assert_eq!(job.total_frames, 300, "10 s at 30 fps");
    fonts_before_frame_zero(&job);
    let ended = export(
        job.clone(),
        &mut Lines::new(Vec::new()),
        Arc::new(Control::default()),
    )
    .await;
    assert!(matches!(ended, Ok(Ended::Done)), "{ended:?}");
    // The worker joins the file in the job's folder; the app's runner moves it into place afterwards.
    let after = ExportJob::open(&job.dir).unwrap();
    assert!(
        after.outputs[0].joined,
        "the format is joined with its sound"
    );
    let file = after.dir.join(&after.outputs[0].file);
    assert!(file.is_file(), "{}", file.display());
    let (frames, length, mean) = decoded(&file, 150);
    assert_eq!(frames, 300, "10 s of frames");
    assert!(
        (length - 10_000_000).abs() <= 100_000,
        "the file lasts 10 s: {length} µs"
    );
    assert!(
        mean.iter().sum::<f64>() > 3.0 * 20.0,
        "the frame at 5 s shows the waves, not black: {mean:?}"
    );
}

/// ProRes gets the 4:2:2 layout the GPU writes (P210) on every OS: prores_ks (Linux, Windows) lists a planar format
/// first, and handing the encoder that one made every exported picture garbage. A flat grey and a flat orange layer,
/// exported as Master (ProRes, the encoder Linux has), come back as those colours. The decode here converts with BT.601
/// while the export is BT.709, so a saturated colour comes back up to about 13 levels off.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_prores_export_keeps_its_colours() {
    use klipolo_engine::api::ExportQuality;
    use klipolo_engine::export::job::ExportJob;
    use klipolo_engine::export::naming::plan_files;
    use klipolo_engine::export::protocol::Lines;
    use klipolo_engine::export::runner::Batch;
    use klipolo_engine::export::shared::Control;
    use klipolo_engine::export::worker::{Ended, export, fonts_before_frame_zero};
    use klipolo_model::contrib::builtin;
    use klipolo_model::params::ParamValue;
    ffmpeg::init().unwrap();
    for (hex, rgb, within) in [
        ("#808080", [128.0, 128.0, 128.0], 4.0),
        ("#C83C1E", [200.0, 60.0, 30.0], 20.0),
    ] {
        let mut p = Project::new("ProRes colours");
        let flat = ItemContent::generator(
            builtin::GENERATOR_GRADIENT.into(),
            BTreeMap::from([
                ("startColor".into(), ParamValue::Text(hex.into())),
                ("endColor".into(), ParamValue::Text(hex.into())),
            ]),
        );
        let mut track = Track::new(TrackKind::Overlay, "Flat");
        track.items.push(Item {
            id: "flat".into(),
            start_us: 0,
            duration_us: 1_000_000,
            attach: None,
            layout: Layout::new(
                Anchor::TopLeft,
                BoxN {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                },
            ),
            content: flat,
            effects: Vec::new(),
        });
        p.timeline.tracks = vec![track];

        let dir = tempfile::tempdir().unwrap();
        let mut files = plan_files(
            &p,
            &[FormatKind::Landscape],
            ExportQuality::Master,
            dir.path(),
        );
        (files[0].width, files[0].height) = (320, 180);
        let batch = Batch {
            project: p,
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
        assert!(matches!(ended, Ok(Ended::Done)), "{hex}: {ended:?}");
        let after = ExportJob::open(&job.dir).unwrap();
        let (frames, _, mean) = decoded(&after.dir.join(&after.outputs[0].file), 15);
        assert_eq!(frames, 30, "{hex}: 1 s of frames");
        for (c, (got, want)) in mean.iter().zip(rgb).enumerate() {
            assert!(
                (got - want).abs() <= within,
                "{hex}: channel {c} came back as {got:.1}, not {want} (all: {mean:?})"
            );
        }
    }
}
