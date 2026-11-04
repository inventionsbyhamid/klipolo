//! The preview holds a short's 9:16 captions while its hook shows, as export does: the app's own path (the engine
//! started on a project feed, its view set to 9:16, a seek), with the hook a kinetic title in its band. Pure: the
//! blank backend records each frame's plan.

use std::sync::Arc;
use std::time::Duration;

use klipolo_engine::api::ViewSpec;
use klipolo_engine::plan::Layer;
use klipolo_engine::session::blank::BlankBackend;
use klipolo_engine::session::{Engine, EngineOptions, ProjectFeed, RenderRequest};
use klipolo_engine::test_support::captions_project;
use klipolo_model::captions::HOOK_BAND;
use klipolo_model::layout::{Anchor, Layout};
use klipolo_model::media::text_motion;
use klipolo_model::project::FormatKind;
use klipolo_model::time::secs_to_us as s;
use klipolo_model::timeline::{Item, ItemContent, Track, TrackKind};
use tokio::sync::watch;

fn options() -> EngineOptions {
    EngineOptions {
        allowed_origins: vec!["tauri://localhost".into()],
        max_preview_pixels: 921_600,
        socket_playback_pixels: 921_600,
        socket_rgba_only: false,
        adapter: "Test".into(),
        gpu_class: klipolo_engine::platform::gpu::GpuClass::Integrated,
    }
}

/// The plan of the frame the preview draws after a seek to `t` s.
async fn after_seek(
    engine: &Engine,
    log: &std::sync::Mutex<Vec<RenderRequest>>,
    t: f64,
    seq: u32,
) -> Vec<Layer> {
    engine.seek(s(t), seq);
    for _ in 0..200 {
        let found = (log.lock().unwrap().iter().rev())
            .find(|r| r.plan.at_us == s(t))
            .map(|r| r.plan.layers.clone());
        if let Some(layers) = found {
            return layers;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no frame at {t} s within 2 s");
}

#[tokio::test]
async fn the_preview_holds_the_9x16_captions_while_a_shorts_hook_shows() {
    // A word every half second, word pop's captions on, 9:16 the main format, and the kinetic hook (the title
    // template) in its band from 0.1 s to 2.1 s.
    let words: Vec<(String, f64, f64)> = (0..20)
        .map(|k| (format!("w{k}"), 0.2 + 0.5 * k as f64, 0.5 + 0.5 * k as f64))
        .collect();
    let said: Vec<(&str, f64, f64)> = words.iter().map(|(w, a, b)| (w.as_str(), *a, *b)).collect();
    let (mut p, speech) = captions_project("caption.word-pop", &said);
    p.settings.primary_format = FormatKind::Portrait;
    let mut track = Track::new(TrackKind::Overlay, "Words");
    track.items = vec![Item {
        id: "hook".into(),
        start_us: s(0.1),
        duration_us: s(2.0),
        attach: None,
        layout: Layout::new(Anchor::TopCenter, HOOK_BAND),
        content: serde_json::from_value::<ItemContent>(serde_json::json!({
            "type": "generator",
            "source": text_motion::SOURCE,
            "params": {"title": "THE HOOK"},
        }))
        .unwrap(),
        effects: Vec::new(),
    }];
    p.timeline.tracks.push(track);
    let (feed, rx) = watch::channel(None);
    let (backend, log, _resets) = BlankBackend::recording();
    let engine = Engine::start(options(), rx, backend).await.unwrap();
    engine.set_view(ViewSpec {
        format: FormatKind::Portrait,
        width: 36,
        height: 64,
    });
    feed.send_replace(Some(ProjectFeed {
        project_id: p.id.clone(),
        revision: 1,
        opened: 1,
        project: Arc::new(p),
        speech: Arc::new(speech),
        text_language: "en".into(),
        registry: klipolo_engine::registry::Registry::builtin(),
    }));
    let captions = |layers: &[Layer]| layers.iter().any(|l| matches!(l, Layer::Caption(_)));
    // Under the hook: none.
    for (k, t) in [0.5, 1.4, 2.0].into_iter().enumerate() {
        let layers = after_seek(&engine, &log, t, k as u32 + 1).await;
        assert!(!captions(&layers), "captions at {t} s");
    }
    // Once it ends: back.
    let layers = after_seek(&engine, &log, 2.5, 9).await;
    assert!(captions(&layers), "no captions at 2.5 s");
}
