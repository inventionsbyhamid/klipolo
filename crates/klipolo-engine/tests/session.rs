mod support;

#[path = "session/held.rs"]
mod held;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use klipolo_engine::api::{
    CodedError, EngineEvent, RATE_UNAVAILABLE, StateReason, ViewSpec, waiting_notice,
};
use klipolo_engine::plan::{BackgroundPlan, MediaCopy, MediaMap, VideoRole, last_frame_us};
use klipolo_engine::present::Output;
use klipolo_engine::session::blank::BlankBackend;
use klipolo_engine::session::{
    Engine, EngineOptions, EngineSlot, PreviewBackend, ProjectFeed, RenderRequest, Rendered,
    UNAVAILABLE,
};
use klipolo_model::Op;
use klipolo_model::project::{Background, BackgroundKind, FormatKind, Project};
use klipolo_model::timeline::{Clip, ClipSource, ItemContent};
use support::*;
use tokio::sync::watch;

struct Harness {
    engine: Engine,
    feed: watch::Sender<Option<ProjectFeed>>,
    log: Arc<Mutex<Vec<RenderRequest>>>,
    resets: Arc<AtomicU32>,
    events: Arc<Mutex<Vec<EngineEvent>>>,
    audio: Arc<Mutex<Vec<&'static str>>>,
    warmed: Arc<Mutex<Vec<(VideoRole, String)>>>,
}

impl Harness {
    /// How many times playback asked the backend for sound (`start_audio`).
    fn backend_audio_starts(&self) -> usize {
        self.audio
            .lock()
            .unwrap()
            .iter()
            .filter(|e| **e == "start")
            .count()
    }
}

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

async fn harness() -> Harness {
    harness_with(options()).await
}

async fn harness_with(options: EngineOptions) -> Harness {
    let (feed, rx) = watch::channel(None);
    let (backend, log, resets) = BlankBackend::recording();
    let audio = backend.audio_log();
    let warmed = backend.warm_log();
    let engine = Engine::start(options, rx, backend).await.unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    engine.set_event_sink(Arc::new(move |e| sink.lock().unwrap().push(e)));
    engine.set_view(ViewSpec {
        format: FormatKind::Landscape,
        width: 64,
        height: 36,
    });
    Harness {
        engine,
        feed,
        log,
        resets,
        events,
        audio,
        warmed,
    }
}

fn publish(h: &Harness, id: &str, revision: u64, p: Project) {
    publish_opened(h, id, revision, 1, p);
}

/// `opened` is the project session's open count: create and open bump it, edits keep it.
fn publish_opened(h: &Harness, id: &str, revision: u64, opened: u64, p: Project) {
    h.feed.send_replace(Some(ProjectFeed {
        project_id: id.into(),
        revision,
        opened,
        project: Arc::new(p),
        speech: Arc::default(),
        text_language: "en".into(),
        registry: klipolo_engine::registry::Registry::builtin(),
    }));
}

async fn eventually(mut ok: impl FnMut() -> bool) {
    for _ in 0..200 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met within 2 s");
}

fn last(h: &Harness) -> Option<RenderRequest> {
    h.log.lock().unwrap().last().cloned()
}

fn solid(hex: &str) -> Background {
    Background {
        kind: BackgroundKind::Solid,
        color: hex.into(),
        ..Background::default()
    }
}

fn colour(h: &Harness) -> Option<BackgroundPlan> {
    last(h).map(|r| r.plan.background)
}

/// A finished editing copy reaches the preview's next still, at the same time; a map that arrives while
/// playing waits for playback to stop (a decoder swap mid-play would stall a frame).
#[tokio::test]
async fn editing_copies_reach_the_next_still_and_wait_for_playback_to_stop() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    h.engine.seek(secs(25.0), 1);
    let shown = |h: &Harness| {
        last(h).and_then(|r| {
            videos(&r.plan)
                .first()
                .map(|v| (v.source.path.to_string(), v.source.time_us))
        })
    };
    eventually(|| shown(&h) == Some(("/media/broll".into(), secs(5.0)))).await;
    let map = |copy: &str| {
        Arc::new(MediaMap::from([(
            "/media/broll".into(),
            MediaCopy::new(copy, 30.0),
        )]))
    };
    h.engine.set_media(map("/cache/broll-1.mp4"));
    eventually(|| shown(&h) == Some(("/cache/broll-1.mp4".into(), secs(5.0)))).await;
    h.engine.play(secs(25.0), 1.0, 2).await.unwrap();
    h.engine.set_media(map("/cache/broll-2.mp4"));
    let frames = h.log.lock().unwrap().len();
    eventually(|| h.log.lock().unwrap().len() > frames + 3).await;
    assert_eq!(
        shown(&h).unwrap().0,
        "/cache/broll-1.mp4",
        "playback keeps its files"
    );
    h.engine.pause().await;
    eventually(|| shown(&h).is_some_and(|(path, _)| path == "/cache/broll-2.mp4")).await;
}

#[tokio::test]
async fn renders_when_the_project_arrives() {
    let h = harness().await;
    let mut p = project();
    p.background = solid("#102030");
    publish(&h, "p1", 0, p);
    eventually(|| {
        colour(&h)
            == Some(BackgroundPlan::Solid {
                color: [0x10, 0x20, 0x30],
            })
    })
    .await;
    let r = last(&h).unwrap();
    assert_eq!((r.width, r.height, r.plan.at_us), (64, 36, 0));
    assert!(
        h.events
            .lock()
            .unwrap()
            .contains(&EngineEvent::Ready { session: 1 })
    );
}

#[tokio::test]
async fn transient_preview_renders_then_expires_on_the_next_revision() {
    let h = harness().await;
    let mut p = project();
    p.background = solid("#000000");
    publish(&h, "p1", 1, p.clone());
    eventually(|| colour(&h) == Some(BackgroundPlan::Solid { color: [0, 0, 0] })).await;
    h.engine.preview(
        "p1".into(),
        1,
        vec![Op::SetBackground {
            background: solid("#FF0000"),
        }],
    );
    eventually(|| colour(&h) == Some(BackgroundPlan::Solid { color: [255, 0, 0] })).await;
    p.name = "Renamed".into();
    publish(&h, "p1", 2, p);
    eventually(|| colour(&h) == Some(BackgroundPlan::Solid { color: [0, 0, 0] })).await;
}

/// A finished transcript (the same revision sent again with a new speech) draws its captions, and a
/// preview dry run that trims 0.5 s off the clip's start (as a drag does) draws "Hi" 0.5 s earlier at once, before any
/// commit. The captions are Clean, whose page holds its 833 ms: 1.0–1.833 s, then 0.5–1.333 s. A dry run pages only the
/// previewed format, so switching the preview to 9:16 mid-drag pages that format again.
#[tokio::test]
async fn a_dry_run_trim_moves_the_page() {
    use std::collections::HashMap;

    use klipolo_engine::plan::Layer;
    use klipolo_model::ops::Edge;
    use klipolo_model::project::{Asset, AssetKind};
    use klipolo_model::speech::{Speech, Transcript, Word, WordKind};

    let h = harness().await;
    let mut p = Project::new("Captions");
    p.captions.enabled = true;
    p.captions.style = Some(klipolo_model::contrib::ContribId::builtin("caption.clean"));
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    p.assets = vec![Asset {
        id: "v".into(),
        kind: AssetKind::Video,
        path: "/media/v.mp4".into(),
        name: "v".into(),
        duration_us: secs(10.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        rotation: None,
        has_audio: true,
        audio_lead_us: None,
        made_with: None,
    }];
    let mut c = Clip::new(ClipSource::Asset { id: "v".into() }, 0, secs(10.0));
    c.id = "c".into();
    p.timeline.main = vec![c];
    let p = Arc::new(p);
    let hi = Word {
        text: "Hi".into(),
        start_us: secs(1.0),
        end_us: secs(1.2),
        confidence: 0.9,
        kind: WordKind::Word,
    };
    let transcript = Transcript {
        key: "k".into(),
        language: "en".into(),
        model: "m".into(),
        covered_us: secs(10.0),
        complete: true,
        words: vec![hi],
    };
    let send = |speech: Speech| {
        h.feed.send_replace(Some(ProjectFeed {
            project_id: "p1".into(),
            revision: 1,
            opened: 1,
            project: p.clone(),
            speech: Arc::new(speech),
            text_language: "en".into(),
            registry: klipolo_engine::registry::Registry::builtin(),
        }));
    };
    // The caption of the frame drawn at `at`, once one is.
    let caption_at = |h: &Harness, at: f64| {
        last(h).filter(|r| r.plan.at_us == secs(at)).map(|r| {
            r.plan.layers.iter().find_map(|l| match l {
                Layer::Caption(c) => Some(c.shown_text()),
                _ => None,
            })
        })
    };
    send(Speech::default());
    h.engine.seek(secs(1.5), 1);
    eventually(|| caption_at(&h, 1.5) == Some(None)).await;
    send(Speech {
        transcripts: HashMap::from([("v".to_owned(), Arc::new(transcript))]),
        ..Speech::default()
    });
    eventually(|| caption_at(&h, 1.5) == Some(Some("Hi".into()))).await;
    h.engine.preview(
        "p1".into(),
        1,
        vec![Op::TrimClip {
            id: "c".into(),
            edge: Edge::Start,
            source_us: secs(0.5),
        }],
    );
    // "Hi" now plays at 0.5–0.7 s: its page ends at 1.333 s, so nothing is up at 1.5 s.
    eventually(|| caption_at(&h, 1.5) == Some(None)).await;
    h.engine.seek(secs(0.6), 2);
    eventually(|| caption_at(&h, 0.6) == Some(Some("Hi".into()))).await;
    h.engine.set_view(ViewSpec {
        format: FormatKind::Portrait,
        width: 36,
        height: 64,
    });
    eventually(|| {
        last(&h).is_some_and(|r| r.height > r.width)
            && caption_at(&h, 0.6) == Some(Some("Hi".into()))
    })
    .await;
}

#[tokio::test]
async fn preview_against_an_old_revision_is_ignored() {
    let h = harness().await;
    publish(&h, "p1", 3, project());
    eventually(|| last(&h).is_some()).await;
    h.engine.preview(
        "p1".into(),
        2,
        vec![Op::SetBackground {
            background: solid("#FF0000"),
        }],
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(colour(&h), Some(BackgroundPlan::Solid { color: [0, 0, 0] }));
}

#[tokio::test]
async fn hidden_window_renders_nothing_until_shown() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    eventually(|| last(&h).is_some()).await;
    h.engine.set_visible(false);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let rendered = h.log.lock().unwrap().len();
    h.engine.seek(secs(5.0), 1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.log.lock().unwrap().len(), rendered);
    h.engine.set_visible(true);
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us == secs(5.0))).await;
}

#[tokio::test]
async fn switching_projects_bumps_the_session_and_resets() {
    let h = harness().await;
    publish(&h, "p1", 4, project());
    h.engine.seek(secs(4.0), 1);
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us == secs(4.0))).await;
    publish(&h, "p2", 0, project());
    eventually(|| {
        h.events
            .lock()
            .unwrap()
            .contains(&EngineEvent::Ready { session: 2 })
    })
    .await;
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us == 0)).await;
    assert!(h.resets.load(Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn seeks_clamp_to_the_last_frame() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    h.engine.seek(secs(999.0), 5);
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us == last_frame_us(secs(30.0), 30))).await;
}

#[tokio::test]
async fn attaching_again_repeats_ready() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    eventually(|| {
        h.events
            .lock()
            .unwrap()
            .contains(&EngineEvent::Ready { session: 1 })
    })
    .await;
    let again = Arc::new(Mutex::new(Vec::new()));
    let sink = again.clone();
    h.engine
        .set_event_sink(Arc::new(move |e| sink.lock().unwrap().push(e)));
    eventually(|| {
        again
            .lock()
            .unwrap()
            .contains(&EngineEvent::Ready { session: 1 })
    })
    .await;
}

#[tokio::test]
async fn closing_the_project_releases_the_backend_and_stops_rendering() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    eventually(|| last(&h).is_some()).await;
    let resets = h.resets.load(Ordering::SeqCst);
    h.feed.send_replace(None);
    eventually(|| h.resets.load(Ordering::SeqCst) > resets).await;
    let rendered = h.log.lock().unwrap().len();
    h.engine.seek(secs(5.0), 1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.log.lock().unwrap().len(), rendered);
}

#[tokio::test]
async fn a_seek_right_after_an_edit_uses_the_edited_project() {
    let h = harness().await;
    let mut short = project();
    short.timeline.main.truncate(1); // 20 s
    publish(&h, "p1", 0, short);
    eventually(|| last(&h).is_some()).await;
    // The edit reaches the feed before the command returns, so a later seek must see the 30 s timeline.
    publish(&h, "p1", 1, project());
    h.engine.seek(secs(25.0), 1);
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us == secs(25.0))).await;
}

#[tokio::test]
async fn reopening_the_open_project_starts_a_new_session() {
    let h = harness().await;
    publish_opened(&h, "p1", 3, 1, project());
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us == 0)).await;
    // Opened again without a close: revisions restart at 0, so frames must belong to a new session.
    publish_opened(&h, "p1", 0, 2, project());
    eventually(|| {
        h.events
            .lock()
            .unwrap()
            .contains(&EngineEvent::Ready { session: 2 })
    })
    .await;
}

#[tokio::test]
async fn a_stale_preview_never_returns_after_reopening() {
    let h = harness().await;
    let mut p = project();
    p.background = solid("#000000");
    publish(&h, "p1", 0, p.clone());
    eventually(|| last(&h).is_some()).await;
    h.engine.preview(
        "p1".into(),
        0,
        vec![Op::SetBackground {
            background: solid("#FF0000"),
        }],
    );
    eventually(|| colour(&h) == Some(BackgroundPlan::Solid { color: [255, 0, 0] })).await;
    publish(&h, "p1", 1, p.clone());
    eventually(|| colour(&h) == Some(BackgroundPlan::Solid { color: [0, 0, 0] })).await;
    h.feed.send_replace(None);
    tokio::time::sleep(Duration::from_millis(50)).await;
    // Revisions restart at 0 on open: the old (p1, 0) preview must not match again.
    publish(&h, "p1", 0, p);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(colour(&h), Some(BackgroundPlan::Solid { color: [0, 0, 0] }));
}

#[tokio::test]
async fn a_burst_of_view_changes_renders_once() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    eventually(|| last(&h).is_some()).await;
    let before = h.log.lock().unwrap().len();
    for i in 1..=40 {
        h.engine.set_view(ViewSpec {
            format: FormatKind::Landscape,
            width: 64 + 2 * i,
            height: 36,
        });
    }
    eventually(|| last(&h).is_some_and(|r| r.width == 144)).await;
    let renders = h.log.lock().unwrap().len() - before;
    assert!(renders <= 2, "{renders} renders for one burst");
}

#[tokio::test]
async fn views_are_held_to_the_pixel_budget() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    h.engine.set_view(ViewSpec {
        format: FormatKind::Landscape,
        width: 4000,
        height: 2000,
    });
    eventually(|| last(&h).is_some_and(|r| r.width > 64)).await;
    let r = last(&h).unwrap();
    assert!(r.width * r.height <= 921_600, "{}x{}", r.width, r.height);
    assert!((f64::from(r.width) / f64::from(r.height) - 2.0).abs() < 0.01);
    assert!(r.width * r.height > 900_000, "{}x{}", r.width, r.height);
}

fn short_project() -> Project {
    let mut p = project();
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "broll".into() },
        0,
        secs(0.5),
    )];
    p
}

fn state(h: &Harness, playing: bool, reason: StateReason) -> Option<i64> {
    h.events.lock().unwrap().iter().rev().find_map(|e| match e {
        EngineEvent::State {
            playing: p,
            at_us,
            reason: r,
            ..
        } if *p == playing && *r == reason => Some(*at_us),
        _ => None,
    })
}

#[tokio::test]
async fn play_emits_state_and_renders_moving_frames() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    h.engine.play(0, 1.0, 0).await.unwrap();
    assert_eq!(state(&h, true, StateReason::Play), Some(0));
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us >= 66_666)).await;
    assert!(h.engine.pause().await > 0);
}

#[tokio::test]
async fn playing_to_the_end_stops_on_the_last_frame() {
    let h = harness().await;
    publish(&h, "p1", 0, short_project());
    h.engine.play(0, 1.0, 0).await.unwrap();
    let last_frame = last_frame_us(secs(0.5), 30);
    eventually(|| state(&h, false, StateReason::Ended) == Some(last_frame)).await;
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us == last_frame)).await;
}

#[tokio::test]
async fn play_from_the_end_restarts() {
    let h = harness().await;
    publish(&h, "p1", 0, short_project());
    h.engine.play(secs(0.5), 1.0, 0).await.unwrap();
    assert_eq!(state(&h, true, StateReason::Play), Some(0));
}

#[tokio::test]
async fn backward_shuttle_draws_earlier_frames_and_stops_at_the_start() {
    let h = harness().await;
    publish(&h, "p", 1, short_project());
    h.engine.play(secs(1.0), -1.0, 0).await.unwrap();
    eventually(|| state(&h, true, StateReason::Play).is_some()).await;
    // Frames go backward…
    eventually(|| last(&h).is_some()).await;
    let first = last(&h).unwrap().plan.at_us;
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us < first)).await;
    // …and playback ends at 0 by itself.
    eventually(|| state(&h, false, StateReason::Ended) == Some(0)).await;
}

#[tokio::test]
async fn double_speed_plays_silently() {
    let h = harness().await;
    publish(&h, "p", 1, project());
    h.engine.play(0, 2.0, 0).await.unwrap();
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us >= secs(0.6))).await;
    assert_eq!(
        h.backend_audio_starts(),
        0,
        "sound plays at normal speed only"
    );
}

/// The rate each `State` event reported, in order.
fn rates(h: &Harness) -> Vec<(bool, f64)> {
    h.events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            EngineEvent::State { playing, rate, .. } => Some((*playing, *rate)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_rate_change_while_playing_mutes_the_sound_and_reports_the_rate() {
    let h = harness().await;
    publish(&h, "p", 1, project());
    h.engine.play(secs(1.0), 1.0, 0).await.unwrap();
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us > secs(1.0))).await;
    h.engine.play(secs(2.0), 2.0, 0).await.unwrap();
    // The sound stops (faded by the output) and doesn't start again at 2×.
    assert_eq!(*h.audio.lock().unwrap(), ["start", "stop"]);
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us > secs(2.2))).await;
    let at = h.engine.pause().await;
    assert!(at > secs(2.2), "{at}");
    assert_eq!(*h.audio.lock().unwrap(), ["start", "stop"]);
    assert_eq!(rates(&h), [(true, 1.0), (true, 2.0), (false, 1.0)]);
}

#[tokio::test]
async fn an_unsupported_rate_is_refused_with_its_code() {
    let h = harness().await;
    publish(&h, "p", 1, project());
    for rate in [0.0, 4.0, -0.5, f64::NAN] {
        assert_eq!(
            h.engine.play(0, rate, 0).await,
            Err(CodedError::new(RATE_UNAVAILABLE))
        );
    }
    assert!(rates(&h).is_empty(), "nothing started");
}

#[tokio::test]
async fn a_seek_sent_before_a_newer_run_is_ignored() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    h.engine.seek(secs(2.0), 3);
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us == secs(2.0))).await;
    // A shuttle press while playing starts run 5; the UI's seek from before it (4) arrives after it.
    h.engine.play(secs(10.0), 1.0, 5).await.unwrap();
    h.engine.seek(secs(1.0), 4);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let at = h.engine.pause().await;
    assert!(at >= secs(10.0), "the stale seek moved playback to {at}");
    // Newer seeks still apply.
    h.engine.seek(secs(20.0), 6);
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us == secs(20.0))).await;
}

#[tokio::test]
async fn a_reloaded_page_counts_its_seeks_from_the_start_again() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    h.engine.seek(secs(5.0), 40);
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us == secs(5.0))).await;
    // The page reloads and attaches again: its seeks start over at 1.
    h.engine.set_event_sink(Arc::new(|_| {}));
    h.engine.seek(secs(7.0), 1);
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us == secs(7.0))).await;
}

#[tokio::test]
async fn pause_returns_where_it_stopped_and_stops_audio() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    h.engine.play(secs(1.0), 1.0, 0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let at = h.engine.pause().await;
    assert!((secs(1.1)..secs(1.5)).contains(&at), "{at}");
    assert_eq!(state(&h, false, StateReason::Pause), Some(at));
    assert_eq!(*h.audio.lock().unwrap(), ["start", "stop"]);
}

#[tokio::test]
async fn hiding_the_window_pauses() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    h.engine.play(0, 1.0, 0).await.unwrap();
    h.engine.set_visible(false);
    eventually(|| state(&h, false, StateReason::Hidden).is_some()).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let rendered = h.log.lock().unwrap().len();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(h.log.lock().unwrap().len(), rendered);
}

#[tokio::test]
async fn switching_projects_stops_playback() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    h.engine.play(0, 1.0, 0).await.unwrap();
    publish(&h, "p2", 0, project());
    // The new project starts at 0, as the UI's playhead does.
    eventually(|| state(&h, false, StateReason::Pause) == Some(0)).await;
    assert_eq!(h.audio.lock().unwrap().last(), Some(&"stop"));
    assert!(
        h.events
            .lock()
            .unwrap()
            .contains(&EngineEvent::Ready { session: 2 })
    );
}

#[tokio::test]
async fn a_seek_while_playing_continues_from_there() {
    let h = harness().await;
    publish(&h, "p1", 0, project());
    h.engine.play(0, 1.0, 0).await.unwrap();
    h.engine.seek(secs(10.0), 9);
    eventually(|| last(&h).is_some_and(|r| r.plan.at_us >= secs(10.0))).await;
    assert!(h.engine.pause().await >= secs(10.0));
}

/// A renderer whose sources' disk doesn't answer while `silent` names them (`Rendered::not_answering`), with the
/// pool's redraw signal: fired when the disk answers.
struct SilentDisk {
    silent: Arc<Mutex<Vec<Arc<str>>>>,
    redraw: Arc<tokio::sync::Notify>,
    frames: BlankBackend,
}

impl PreviewBackend for SilentDisk {
    async fn render(&mut self, req: RenderRequest) -> Result<Rendered, String> {
        let mut rendered = self.frames.render(req).await?;
        rendered.not_answering = self.silent.lock().unwrap().clone();
        Ok(rendered)
    }
    fn reset(&mut self) {}
    fn redraw(&self) -> Option<Arc<tokio::sync::Notify>> {
        Some(self.redraw.clone())
    }
}

/// A source whose disk doesn't answer is drawn without and not said at first (a disk spinning up answers
/// in a moment); silent for `WAITING_AFTER`, the preview gets the neutral waiting note, which goes when the disk
/// answers and the still is drawn again without waiting for it. Paused time: the 10 s pass at once.
#[tokio::test(start_paused = true)]
async fn a_disk_silent_for_ten_seconds_is_waited_for_aloud_until_it_answers() {
    let file: Arc<str> = "/Volumes/Archive/intro.mp4".into();
    let silent = Arc::new(Mutex::new(vec![file.clone()]));
    let redraw = Arc::new(tokio::sync::Notify::new());
    let (frames, log, _) = BlankBackend::recording();
    let backend = SilentDisk {
        silent: silent.clone(),
        redraw: redraw.clone(),
        frames,
    };
    let (feed, rx) = watch::channel(None);
    let engine = Engine::start(options(), rx, backend).await.unwrap();
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
    let notes = || {
        events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                EngineEvent::Waiting { notice } => Some(notice.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    eventually(|| !log.lock().unwrap().is_empty()).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(notes().is_empty(), "under 10 s, a silent disk isn't said");
    tokio::time::sleep(Duration::from_secs(6)).await;
    eventually(|| notes() == [Some(waiting_notice(&file))]).await;
    // The disk answers: the pool's signal draws the still again, without waiting, and the note goes.
    silent.lock().unwrap().clear();
    redraw.notify_one();
    eventually(|| notes() == [Some(waiting_notice(&file)), None]).await;
}

/// A renderer with a bug: its first frame panics.
struct Panicking;

impl PreviewBackend for Panicking {
    async fn render(&mut self, _req: RenderRequest) -> Result<Rendered, String> {
        panic!("injected: the renderer panicked");
    }
    fn reset(&mut self) {}
}

#[tokio::test]
async fn an_engine_that_panics_says_so_and_the_next_attach_replaces_it() {
    let slot = EngineSlot::new();
    let (feed, rx) = watch::channel(None);
    let engine = slot
        .get_or_start(|| Engine::start(options(), rx, Panicking))
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
    // The UI is told, whatever the presenter: the preview note and its Retry appear (no frozen frame).
    eventually(|| {
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, EngineEvent::Error { message } if message.as_str() == UNAVAILABLE))
    })
    .await;
    assert!(!engine.is_running());
    // Retry attaches again: a new engine.
    let again = slot
        .get_or_start(|| async {
            let (_feed, rx) = watch::channel(None);
            Engine::start(options(), rx, BlankBackend::new()).await
        })
        .await
        .unwrap();
    assert_ne!(again.info().token, engine.info().token);
}

#[tokio::test]
async fn an_ended_engine_is_replaced_on_the_next_attach() {
    let slot = EngineSlot::new();
    let start = || async {
        let (_feed, rx) = watch::channel(None);
        Engine::start(options(), rx, BlankBackend::new()).await
    };
    let first = slot.get_or_start(start).await.unwrap();
    assert_eq!(slot.get_or_start(start).await.unwrap().info(), first.info());
    // The actor ends (here shut down; in the app, a panic): the next attach gets a working engine.
    first.shutdown().await;
    eventually(|| slot.running().is_none()).await;
    let second = slot.get_or_start(start).await.unwrap();
    assert_ne!(second.info().token, first.info().token);
    assert!(second.is_running());
}

#[tokio::test]
async fn overlay_video_decoders_open_before_the_overlay_starts() {
    let h = harness().await;
    let mut p = project();
    p.timeline.tracks = vec![overlay_track(
        "o",
        vec![item(
            "pip",
            0.5,
            2.0,
            bx(0.6, 0.6, 0.3, 0.3),
            ItemContent::Video {
                asset_id: "broll".into(),
                source_in_us: 0,
                speed: 1.0,
                volume: 1.0,
                muted: false,
                mirror: false,
                tie: None,
            },
        )],
    )];
    publish(&h, "p1", 0, p);
    h.engine.play(0, 1.0, 0).await.unwrap();
    let overlay = (VideoRole::Overlay, "broll".to_string());
    eventually(|| h.warmed.lock().unwrap().contains(&overlay)).await;
    let first = h.log.lock().unwrap().first().map(|r| r.plan.at_us);
    assert!(
        first.is_some_and(|t| t < secs(0.5)),
        "warmed ahead of {first:?}"
    );
    h.engine.pause().await;
}

#[tokio::test]
async fn a_long_zoom_previewed_then_committed_stays_zoomed() {
    use klipolo_model::ops::ItemPatch;
    use klipolo_model::timeline::ZoomTarget;
    let h = harness().await;
    let mut p = project();
    // A 10-minute zoom: its springs take long enough to simulate that they're built off the actor.
    lengthen(&mut p, 700.0);
    p.timeline.main[0].source_out_us = secs(650.0);
    let zoom = |scale| ItemContent::Zoom {
        target: ZoomTarget::Auto { using: vec![] },
        scale,
    };
    p.timeline.tracks = vec![zoom_track(vec![item(
        "z",
        1.0,
        600.0,
        bx(0.0, 0.0, 1.0, 1.0),
        zoom(2.0),
    )])];
    let width_at_20s = |h: &Harness| {
        last(h)
            .filter(|r| r.plan.at_us == secs(20.0))
            .map(|r| videos(&r.plan)[0].crop.w)
    };
    publish(&h, "p1", 1, p.clone());
    h.engine.seek(secs(20.0), 1);
    eventually(|| width_at_20s(&h).is_some_and(|w| (w - 0.5).abs() < 0.01)).await;
    // Preview a new scale (its rebuild starts off the actor) and commit it while that rebuild runs, as releasing a
    // slider does.
    let op = Op::UpdateItem {
        id: "z".into(),
        patch: ItemPatch {
            content: Some(zoom(1.5)),
            ..ItemPatch::default()
        },
    };
    let rendered = h.log.lock().unwrap().len();
    h.engine.preview("p1".into(), 1, vec![op.clone()]);
    eventually(|| h.log.lock().unwrap().len() > rendered).await;
    publish(&h, "p1", 2, klipolo_model::apply_ops(&p, &[op]).unwrap());
    // Once the rebuild finishes, the committed zoom draws.
    eventually(|| width_at_20s(&h).is_some_and(|w| (w - 1.0 / 1.5).abs() < 0.01)).await;
}

/// A portrait still of a take whose frames show the cursor (Burned: nothing else changes when it
/// loads) is drawn again when its follow path lands, so the crop follows the cursor without a scrub.
#[tokio::test]
async fn a_follow_path_landing_redraws_the_paused_still() {
    use klipolo_cursor::format::{Capture, Line};
    use klipolo_model::cursor_style::CursorStyle;
    let h = harness().await;
    h.engine.set_view(ViewSpec {
        format: FormatKind::Portrait,
        width: 36,
        height: 64,
    });
    // The cursor moves to the right in the first half second and rests there.
    let dir = tempfile::tempdir().unwrap();
    let mut lines = vec![support::takes::header(Capture::Burned)];
    lines.extend((0..=120).map(|k| Line::Move {
        t: k * 500_000,
        x: if k == 0 { 0.5 } else { 0.8 },
        y: 0.5,
    }));
    let mut p = support::recording::cursor_project(dir.path(), &lines, CursorStyle::default());
    p.settings.formats = vec![FormatKind::Landscape, FormatKind::Portrait];
    // Its file arrives after the first still (a drive mounted late), so the take lands on a drawn still.
    let file = std::path::PathBuf::from(&p.recordings[0].cursor.as_ref().unwrap().path);
    let away = file.with_extension("later");
    std::fs::rename(&file, &away).unwrap();
    let centre = |h: &Harness| {
        last(h)
            .filter(|r| r.plan.at_us == secs(10.0) && r.plan.format == FormatKind::Portrait)
            .map(|r| {
                let c = videos(&r.plan)[0].crop;
                c.x + c.w / 2.0
            })
    };
    publish(&h, "p1", 1, p.clone());
    h.engine.seek(secs(10.0), 1);
    eventually(|| centre(&h).is_some_and(|x| (x - 0.5).abs() < 1e-6)).await;
    std::fs::rename(&away, &file).unwrap();
    publish(&h, "p1", 2, p);
    eventually(|| centre(&h).is_some_and(|x| (x - 0.8).abs() < 0.07)).await;
}

#[tokio::test]
async fn a_sound_device_that_fails_warns_and_plays_on_without_sound() {
    let (feed, rx) = watch::channel(None);
    let (backend, log, _) = BlankBackend::recording();
    let engine = Engine::start(options(), rx, backend.with_failing_audio())
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
    let warned = || {
        events.lock().unwrap().contains(&EngineEvent::Warning {
            message: "Sound isn't available right now. Playing without sound.".into(),
        })
    };
    eventually(warned).await;
    let moving = |log: &Arc<Mutex<Vec<RenderRequest>>>| {
        log.lock()
            .unwrap()
            .last()
            .is_some_and(|r| r.plan.at_us >= 66_666)
    };
    eventually(|| moving(&log)).await;
    assert!(engine.pause().await > 0);
}

#[tokio::test]
async fn play_before_the_preview_has_a_size_starts_once_it_does() {
    let (feed, rx) = watch::channel(None);
    let (backend, log, _) = BlankBackend::recording();
    let engine = Engine::start(options(), rx, backend).await.unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    engine.set_event_sink(Arc::new(move |e| sink.lock().unwrap().push(e)));
    feed.send_replace(Some(ProjectFeed {
        project_id: "p1".into(),
        revision: 0,
        opened: 1,
        project: Arc::new(project()),
        speech: Arc::default(),
        text_language: "en".into(),
        registry: klipolo_engine::registry::Registry::builtin(),
    }));
    engine.play(secs(1.0), 1.0, 0).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(log.lock().unwrap().is_empty(), "nothing to draw into yet");
    assert!(
        !events
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, EngineEvent::State { playing: false, .. }))
    );
    engine.set_view(ViewSpec {
        format: FormatKind::Landscape,
        width: 64,
        height: 36,
    });
    eventually(|| {
        log.lock()
            .unwrap()
            .last()
            .is_some_and(|r| r.plan.at_us > secs(1.0))
    })
    .await;
    assert!(engine.pause().await > secs(1.0));
}

/// A play still waiting for the preview's size, called off by opening another project, says it stopped (at the new
/// project's 0), so nothing that heard it start (the UI, the app's busy registry) is left thinking it plays.
#[tokio::test]
async fn a_waiting_play_says_it_stopped_when_another_project_opens() {
    let (feed, rx) = watch::channel(None);
    let (backend, _, _) = BlankBackend::recording();
    let engine = Engine::start(options(), rx, backend).await.unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    engine.set_event_sink(Arc::new(move |e| sink.lock().unwrap().push(e)));
    let open = |id: &str| ProjectFeed {
        project_id: id.into(),
        revision: 0,
        opened: 1,
        project: Arc::new(project()),
        speech: Arc::default(),
        text_language: "en".into(),
        registry: klipolo_engine::registry::Registry::builtin(),
    };
    feed.send_replace(Some(open("p1")));
    engine.play(secs(1.0), 1.0, 0).await.unwrap();
    feed.send_replace(Some(open("p2")));
    let stopped = |e: &EngineEvent| {
        matches!(
            e,
            EngineEvent::State {
                playing: false,
                at_us: 0,
                reason: StateReason::Pause,
                ..
            }
        )
    };
    eventually(|| events.lock().unwrap().iter().any(stopped)).await;
}

#[tokio::test]
async fn socket_playback_is_nv12_within_its_budget_and_a_pause_draws_the_full_still() {
    let h = harness_with(EngineOptions {
        socket_playback_pixels: 32 * 18,
        ..options()
    })
    .await;
    publish(&h, "p1", 0, project());
    eventually(|| last(&h).is_some_and(|r| r.output == Output::Rgba && r.width == 64)).await;
    h.engine.play(0, 1.0, 0).await.unwrap();
    eventually(|| last(&h).is_some_and(|r| r.output == Output::Nv12)).await;
    let r = last(&h).unwrap();
    assert_eq!(
        (r.width, r.height),
        (32, 18),
        "playback stays within the socket's budget"
    );
    h.engine.pause().await;
    eventually(|| last(&h).is_some_and(|r| r.output == Output::Rgba && r.width == 64)).await;
}

/// Native init failure → the socket keeps drawing (no blank preview): the blank backend has no GPU to present with.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_native_start_that_fails_leaves_the_socket_drawing() {
    use futures::StreamExt;
    use klipolo_engine::api::Presenter;
    use klipolo_engine::frames::trailer::read_trailer;
    use klipolo_engine::platform::NativeSurface;
    use klipolo_engine::present::macos::NativeLayer;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let h = harness().await;
    publish(&h, "p1", 0, project());
    let info = h.engine.info();
    assert_eq!(info.presenter, Presenter::Socket);
    assert!(
        h.engine
            .attach_native(NativeSurface::Metal(NativeLayer::detached()))
            .await
            .is_err()
    );
    eventually(|| last(&h).is_some()).await;
    let mut req = format!("ws://127.0.0.1:{}/frames?t={}", info.port, info.token)
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Origin", "tauri://localhost".parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("a frame over the socket")
        .unwrap()
        .unwrap();
    assert_eq!(read_trailer(&msg.into_data()).unwrap().session, 1);
    assert_eq!(last(&h).unwrap().output, Output::Rgba);
}

/// While a title is typed on the canvas, the editor draws it, so the preview's frames
/// leave out that one text layer (stills and playback), and bring it back once the UI says the edit is over.
#[tokio::test]
async fn the_preview_leaves_out_the_text_being_edited_and_only_that_one() {
    use klipolo_engine::plan::Layer;

    fn texts(r: &RenderRequest) -> Vec<String> {
        r.plan
            .layers
            .iter()
            .filter_map(|l| match l {
                Layer::Text(t) => Some(t.id.to_string()),
                _ => None,
            })
            .collect()
    }
    async fn shows(h: &Harness, want: &[&str]) {
        for _ in 0..200 {
            if last(h).is_some_and(|r| texts(&r) == want) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "wanted {want:?}, the last frame has {:?}",
            last(h).map(|r| texts(&r))
        );
    }

    let h = harness().await;
    let mut p = project();
    let rect = bx(0.1, 0.1, 0.5, 0.2);
    // One title per track: a track holds one item at a time.
    p.timeline.tracks = vec![
        overlay_track("t1", vec![item("a", 0.0, 5.0, rect, text("Edited"))]),
        overlay_track("t2", vec![item("b", 0.0, 5.0, rect, text("Kept"))]),
    ];
    publish(&h, "p1", 0, p);
    h.engine.seek(secs(1.0), 1);
    shows(&h, &["a", "b"]).await;

    h.engine.set_editing(Some("a".into()));
    shows(&h, &["b"]).await;
    // Playing frames leave it out too (playing finishes the edit, but its end may still be on its way).
    h.engine.play(secs(1.0), 1.0, 2).await.unwrap();
    let frames = h.log.lock().unwrap().len();
    eventually(|| h.log.lock().unwrap().len() > frames + 3).await;
    assert!(
        h.log.lock().unwrap()[frames..]
            .iter()
            .all(|r| texts(r) == ["b"]),
        "every playback frame leaves the edited title out"
    );
    h.engine.pause().await;

    h.engine.set_editing(None);
    shows(&h, &["a", "b"]).await;
}

/// The same revision sent again with a new registry (the app's snapshot checked an effect) is drawn with it;
/// until then the effect the catalogue knows is noted as being checked, not as missing.
#[tokio::test]
async fn a_new_registry_for_the_same_revision_draws_with_it() {
    use klipolo_engine::plan::Layer;
    use klipolo_engine::registry::{Registry, ShaderEffect, ShaderKind};
    use klipolo_model::contrib::{ContribDef, ContribId, ContribKind};
    use klipolo_model::layout::{Anchor, BoxN, Layout};
    use klipolo_model::timeline::{Item, Track, TrackKind};
    let h = harness().await;
    let id = ContribId::new("lib.k3f9q2ma", "generator.glow.v1");
    let def = ContribDef::new(id.as_str(), ContribKind::Generator, "Glow");
    let mut p = project();
    let mut track = Track::new(TrackKind::Overlay, "Layers");
    track.items.push(Item {
        id: "glow".into(),
        start_us: 0,
        duration_us: secs(2.0),
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
        content: ItemContent::generator(id.clone(), Default::default()),
        effects: Vec::new(),
    });
    p.timeline.tracks.push(track);
    let p = Arc::new(p);
    let send = |registry: Arc<Registry>| {
        h.feed.send_replace(Some(ProjectFeed {
            project_id: "p1".into(),
            revision: 3,
            opened: 1,
            project: p.clone(),
            speech: Arc::default(),
            text_language: "en".into(),
            registry,
        }));
    };
    send(Arc::new(Registry::with_definitions(
        &Registry::builtin(),
        [&def],
    )));
    eventually(|| last(&h).is_some_and(|r| r.plan.missing.iter().any(|m| m.checking))).await;
    let body = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }";
    let shader = ShaderEffect::assemble_untrusted(&def, ShaderKind::Generator, body).unwrap();
    send(Registry::builtin_with_test_shader(&def, shader));
    eventually(|| {
        last(&h).is_some_and(|r| {
            r.plan
                .layers
                .iter()
                .any(|l| matches!(l, Layer::Generator(_)))
        })
    })
    .await;
}
