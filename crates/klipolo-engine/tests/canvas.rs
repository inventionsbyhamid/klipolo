//! A video lasts until the later of the main track's end and the latest visual
//! layer's end. Past the main track's end (and in a gap before a layer) a frame is the background with the layers on
//! top; playback, seeks and an export's frame count run to the full length. GPU pixels: `gpu.rs`
//! (`a_canvas_of_only_layers_draws_them_at_five_seconds`).

mod support;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use klipolo_engine::api::{EngineEvent, StateReason, ViewSpec};
use klipolo_engine::export::jobs::frames_of;
use klipolo_engine::plan::{Layer, PlanContext, frame_plan, last_frame_us};
use klipolo_engine::session::blank::BlankBackend;
use klipolo_engine::session::{Engine, ProjectFeed, RenderRequest};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::timeline::{Clip, ClipSource, ItemContent, Track, TrackKind};
use support::*;
use tokio::sync::watch;

fn waves() -> ItemContent {
    ItemContent::generator("klipolo.builtin/generator.waves".into(), BTreeMap::new())
}

/// A canvas of layers alone: an empty project with a Waves layer 0–10 s and a text 0–10 s.
fn canvas() -> Project {
    let mut p = project();
    p.timeline.main.clear();
    p.timeline.tracks = vec![
        overlay_track(
            "bg",
            vec![item("bg", 0.0, 10.0, bx(0.0, 0.0, 1.0, 1.0), waves())],
        ),
        overlay_track(
            "titles",
            vec![item(
                "title",
                0.0,
                10.0,
                bx(0.1, 0.4, 0.8, 0.2),
                text("Hello"),
            )],
        ),
    ];
    p
}

/// A 5 s main track (b-roll 0–5 s) and a text from `start` for `len` seconds.
fn five_and(start: f64, len: f64) -> Project {
    let mut p = project();
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "broll".into() },
        0,
        secs(5.0),
    )];
    p.timeline.tracks = vec![overlay_track(
        "titles",
        vec![item(
            "title",
            start,
            len,
            bx(0.1, 0.4, 0.8, 0.2),
            text("Hello"),
        )],
    )];
    p
}

fn kinds(p: &Project, at: f64) -> Vec<&'static str> {
    frame_plan(
        &PlanContext::new(Arc::new(p.clone())),
        secs(at),
        FormatKind::Landscape,
    )
    .layers
    .iter()
    .map(|l| match l {
        Layer::Video(_) => "video",
        Layer::Generator(_) => "generator",
        Layer::Text(_) => "text",
        _ => "other",
    })
    .collect()
}

#[test]
fn a_canvas_exports_ten_seconds_of_frames_showing_its_layers() {
    let p = canvas();
    assert_eq!(frames_of(&p, 30), 300);
    assert_eq!(kinds(&p, 5.0), ["generator", "text"]);
    // A song longer than the main track doesn't add frames.
    let mut p = five_and(1.0, 2.0);
    let mut music = Track::new(TrackKind::Audio, "Music");
    music.items.push(item(
        "song",
        0.0,
        60.0,
        bx(0.0, 0.0, 1.0, 1.0),
        ItemContent::Audio {
            asset_id: "music".into(),
            source_in_us: 0,
            volume: 1.0,
            muted: false,
            fade_in_us: 0,
            fade_out_us: 0,
            effect: None,
            drift_ppm: 0.0,
            centre_from: None,
            tie: None,
            duck: None,
            volume_keys: Vec::new(),
        },
    ));
    p.timeline.tracks.push(music);
    assert_eq!(frames_of(&p, 30), 150);
}

#[test]
fn past_the_main_track_a_frame_is_the_background_with_the_layers() {
    let p = five_and(3.0, 5.0);
    assert_eq!(frames_of(&p, 30), 240, "8 s");
    assert_eq!(kinds(&p, 4.0), ["video", "text"]);
    assert_eq!(kinds(&p, 6.5), ["text"], "no main footage past its end");
    // A layer after a gap: the gap is the background alone.
    let p = five_and(7.0, 2.0);
    assert_eq!(frames_of(&p, 30), 270, "9 s");
    assert!(kinds(&p, 6.0).is_empty());
    assert_eq!(kinds(&p, 8.0), ["text"]);
}

struct Harness {
    engine: Engine,
    feed: watch::Sender<Option<ProjectFeed>>,
    log: Arc<Mutex<Vec<RenderRequest>>>,
    events: Arc<Mutex<Vec<EngineEvent>>>,
}

async fn harness(p: Project) -> Harness {
    let (feed, rx) = watch::channel(None);
    let (backend, log, _) = BlankBackend::recording();
    let engine = Engine::start(
        klipolo_engine::session::EngineOptions {
            allowed_origins: vec!["tauri://localhost".into()],
            max_preview_pixels: 921_600,
            socket_playback_pixels: 921_600,
            socket_rgba_only: false,
            adapter: "Test".into(),
            gpu_class: klipolo_engine::platform::gpu::GpuClass::Integrated,
        },
        rx,
        backend,
    )
    .await
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    engine.set_event_sink(Arc::new(move |e| sink.lock().unwrap().push(e)));
    engine.set_view(ViewSpec {
        format: FormatKind::Landscape,
        width: 64,
        height: 36,
    });
    feed.send_replace(Some(ProjectFeed {
        project_id: "p1".into(),
        revision: 0,
        opened: 1,
        project: Arc::new(p),
        speech: Arc::default(),
        text_language: "en".into(),
        registry: klipolo_engine::registry::Registry::builtin(),
    }));
    Harness {
        engine,
        feed,
        log,
        events,
    }
}

async fn eventually(mut ok: impl FnMut() -> bool) {
    for _ in 0..300 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met within 3 s");
}

fn last_at(h: &Harness) -> Option<i64> {
    h.log.lock().unwrap().last().map(|r| r.plan.at_us)
}

#[tokio::test]
async fn seeks_reach_the_end_of_the_last_layer() {
    let h = harness(five_and(3.0, 5.0)).await;
    h.engine.seek(secs(6.5), 1);
    eventually(|| last_at(&h) == Some(secs(6.5))).await;
    h.engine.seek(secs(999.0), 2);
    eventually(|| last_at(&h) == Some(last_frame_us(secs(8.0), 30))).await;
    drop(h.feed);
}

#[tokio::test]
async fn a_canvas_of_only_layers_plays_to_its_end() {
    let mut p = canvas();
    for t in &mut p.timeline.tracks {
        t.items[0].duration_us = secs(0.5);
    }
    let h = harness(p).await;
    h.engine.play(0, 1.0, 0).await.expect("a canvas plays");
    let end = last_frame_us(secs(0.5), 30);
    eventually(|| {
        h.events.lock().unwrap().iter().any(|e| {
            matches!(e, EngineEvent::State { playing: false, at_us, reason: StateReason::Ended, .. } if *at_us == end)
        })
    })
    .await;
}
