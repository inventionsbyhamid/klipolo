//! Playback that can't keep up says so (`EngineEvent::FramesLate`), once per run, so transcription
//! holds for the rest of it. A backend whose every frame takes 4 frames' time stands in for a busy computer.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use klipolo_engine::api::{EngineEvent, ViewSpec};
use klipolo_engine::session::blank::BlankBackend;
use klipolo_engine::session::{
    Engine, EngineOptions, PreviewBackend, ProjectFeed, RenderRequest, Rendered,
};
use klipolo_model::project::FormatKind;
use support::project;
use tokio::sync::watch;

/// Draws like the blank backend, 130 ms a frame (30 fps frames are due every 33 ms).
struct Slow(BlankBackend);

impl PreviewBackend for Slow {
    async fn render(&mut self, req: RenderRequest) -> Result<Rendered, String> {
        tokio::time::sleep(Duration::from_millis(130)).await;
        self.0.render(req).await
    }
    fn reset(&mut self) {}
}

fn late(events: &Mutex<Vec<EngineEvent>>) -> usize {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, EngineEvent::FramesLate { .. }))
        .count()
}

#[tokio::test]
async fn playback_that_falls_behind_says_so_once() {
    let (feed, rx) = watch::channel(None);
    let options = EngineOptions {
        allowed_origins: vec!["tauri://localhost".into()],
        max_preview_pixels: 921_600,
        socket_playback_pixels: 921_600,
        socket_rgba_only: false,
        adapter: "Test".into(),
        gpu_class: klipolo_engine::platform::gpu::GpuClass::Integrated,
    };
    let engine = Engine::start(options, rx, Slow(BlankBackend::new()))
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
        project: Arc::new(project()),
        speech: Arc::default(),
        text_language: "en".into(),
        registry: klipolo_engine::registry::Registry::builtin(),
    }));
    engine.play(0, 1.0, 0).await.unwrap();
    for _ in 0..50 {
        if late(&events) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(late(&events), 1, "said within 5 s of playing");
    // Still behind a second later: said once for the run.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(late(&events), 1, "once per run of playback");
    engine.pause().await;
}
