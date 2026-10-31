//! `cargo run -p klipolo-engine --release --example perf_harness -- copy <video file> [<its editing copy>]`: what an
//! editing copy saves while the preview plays. With no copy given, one is made first, in this process, as the app's
//! worker makes it (an Intel Mac's policy), in a folder that goes at the end. Then the real actor and GPU backend play
//! 5 s at 1× four times, each on a fresh engine: the file as the main footage, then the same with the file again as a
//! picture-in-picture layer (a quarter of the frame, 10 s further in), each reading the file itself and then its copy
//! (handed to the preview with `Engine::set_media`, as the app does). Reported per run: frames drawn per second, render
//! ms per frame (pixels read back and waited for, as the harness's playback does) and the VideoToolbox decoder
//! service's CPU per second of video. Nothing is captured and no sound device opens.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use klipolo_engine::api::ViewSpec;
use klipolo_engine::export::protocol::Lines;
use klipolo_engine::export::shared::Control;
use klipolo_engine::media::proxy::decide::{ProxyPolicy, needs_proxy, proxy_rate};
use klipolo_engine::media::proxy::facts::{SourceFacts, probe_facts};
use klipolo_engine::media::proxy::job::ProxyJob;
use klipolo_engine::media::proxy::run::RunOutcome;
use klipolo_engine::media::proxy::worker::run_job;
use klipolo_engine::plan::{MediaCopy, MediaMap};
use klipolo_engine::platform::gpu::GpuClass;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::session::{Engine, EngineOptions, ProjectFeed};
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource, Item, ItemContent, Track, TrackKind};
use tokio::sync::{mpsc, watch};

use super::measure::{mark, report, vt_cpu_ns};
use super::project::secs;
use super::{Probe, Renders};

/// Seconds each run plays.
const PLAY_S: f64 = 5.0;
/// Where each run starts playing (s).
const FROM_S: f64 = 1.0;
/// How much further in the picture-in-picture layer reads the file (s), so its frames aren't the main footage's.
const LAYER_OFFSET_S: f64 = 10.0;

pub async fn run(args: &[String]) {
    let Some(file) = args.first() else {
        eprintln!("usage: perf_harness copy <video file> [<its editing copy>]");
        return;
    };
    let file = std::fs::canonicalize(file)
        .expect("the video file")
        .display()
        .to_string();
    let facts = probe_facts(Path::new(&file)).expect("a video the probe reads");
    let reason = |class| needs_proxy(&facts, &ProxyPolicy::with_encoder(class, true));
    println!(
        "copy: {file}\n  {}×{} {} at {:.3} fps, {:.1} s | needs a copy: Intel {:?}, Apple {:?}",
        facts.width,
        facts.height,
        facts.codec,
        facts.rate.f64(),
        facts.duration_us as f64 / 1e6,
        reason(GpuClass::Integrated),
        reason(GpuClass::Apple),
    );
    let length_s = facts.duration_us as f64 / 1e6;
    assert!(
        length_s >= FROM_S + PLAY_S + LAYER_OFFSET_S,
        "the file is {length_s:.1} s long: it needs at least {} s",
        FROM_S + PLAY_S + LAYER_OFFSET_S
    );
    let scratch = tempfile::tempdir().unwrap();
    let copy = match args.get(1) {
        Some(copy) => PathBuf::from(copy),
        None => make_copy(scratch.path(), &file, &facts).await,
    };
    // The copy runs at the source's exact rate (0: the decoder pool probes it).
    let fps = proxy_rate(&facts).map_or(0.0, |rate| rate.f64());
    let copies: MediaMap = [(
        Arc::<str>::from(file.as_str()),
        MediaCopy::new(copy.to_string_lossy().as_ref(), fps),
    )]
    .into_iter()
    .collect();
    for layer in [false, true] {
        let project = Arc::new(project(&file, &facts, layer));
        let what = if layer {
            "with a picture-in-picture layer"
        } else {
            "main footage only"
        };
        play(&format!("{what}, the file itself"), project.clone(), None).await;
        play(&format!("{what}, its copy"), project, Some(copies.clone())).await;
    }
}

/// Makes `file`'s editing copy in `dir` as the app's worker would on an Intel Mac, and says what it took.
async fn make_copy(dir: &Path, file: &str, facts: &SourceFacts) -> PathBuf {
    let policy = ProxyPolicy::with_encoder(GpuClass::Integrated, true);
    let job = ProxyJob::create(&dir.join("proxies"), file, facts, &policy).expect("a copy job");
    let started = Instant::now();
    let outcome = run_job(
        &job.dir,
        &mut Lines::new(std::io::sink()),
        Arc::new(Control::default()),
    )
    .await;
    assert_eq!(outcome, RunOutcome::Finished, "the copy");
    let elapsed = started.elapsed().as_secs_f64();
    let seconds = job.duration_us as f64 / 1e6;
    let bytes = std::fs::metadata(&job.out).map_or(0, |m| m.len());
    println!(
        "  made a {}×{} copy in {elapsed:.1} s ({:.2}× realtime), {:.1} MB: {}",
        job.width,
        job.height,
        seconds / elapsed,
        bytes as f64 / 1e6,
        job.out.display(),
    );
    job.out.clone()
}

/// `file` on the main track for its whole length; with `layer`, the same file again as a picture-in-picture layer in
/// the bottom right corner, `LAYER_OFFSET_S` further in.
fn project(file: &str, facts: &SourceFacts, layer: bool) -> Project {
    let mut p = Project::new("Copy");
    p.assets = vec![Asset {
        id: "file".into(),
        kind: AssetKind::Video,
        path: file.into(),
        name: "file".into(),
        duration_us: facts.duration_us,
        width: facts.width,
        height: facts.height,
        fps: facts.rate.f64(),
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    }];
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "file".into() },
        0,
        facts.duration_us,
    )];
    if layer {
        let offset: Us = secs(LAYER_OFFSET_S);
        let mut track = Track::new(TrackKind::Overlay, "Picture in picture");
        track.items.push(Item {
            id: "pip".into(),
            start_us: 0,
            duration_us: facts.duration_us - offset,
            attach: None,
            layout: Layout::new(
                Anchor::TopLeft,
                BoxN {
                    x: 0.6,
                    y: 0.6,
                    w: 0.35,
                    h: 0.35,
                },
            ),
            content: ItemContent::Video {
                asset_id: "file".into(),
                source_in_us: offset,
                speed: 1.0,
                volume: 1.0,
                muted: true,
                mirror: false,
                tie: None,
            },
            effects: Vec::new(),
        });
        p.timeline.tracks = vec![track];
    }
    p
}

/// A fresh engine on the GPU backend plays `project` for `PLAY_S` from `FROM_S`, reading the copies in `media`.
async fn play(name: &str, project: Arc<Project>, media: Option<MediaMap>) {
    let stats = Arc::new(Mutex::new(Renders::default()));
    let (done_tx, mut done) = mpsc::unbounded_channel();
    let backend = Probe {
        inner: GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
            .await
            .expect("GPU"),
        stats: stats.clone(),
        done: done_tx,
    };
    let (feed_tx, feed) = watch::channel(None);
    let engine = Engine::start(
        EngineOptions {
            allowed_origins: vec![],
            max_preview_pixels: 1280 * 720,
            socket_playback_pixels: 1280 * 720,
            socket_rgba_only: true,
            adapter: String::new(),
            gpu_class: GpuClass::Integrated,
        },
        feed,
        backend,
    )
    .await
    .unwrap();
    engine.set_event_sink(Arc::new(|_| {}));
    engine.set_view(ViewSpec {
        format: FormatKind::Landscape,
        width: 1280,
        height: 720,
    });
    // Before the project arrives: the first frame already reads the copy.
    if let Some(map) = media {
        engine.set_media(Arc::new(map));
    }
    feed_tx.send_replace(Some(ProjectFeed {
        project_id: project.id.clone(),
        revision: 0,
        opened: 1,
        project,
        speech: Arc::default(),
        text_language: "en".into(),
        registry: klipolo_engine::registry::Registry::builtin(),
    }));
    tokio::time::timeout(Duration::from_secs(10), done.recv())
        .await
        .expect("first frame");
    // The still at the start first, so playback starts with its decoders there (as after a click).
    engine.seek(secs(FROM_S), 1);
    tokio::time::sleep(Duration::from_millis(500)).await;
    while done.try_recv().is_ok() {}
    *stats.lock().unwrap_or_else(PoisonError::into_inner) = Renders::default();
    let start = mark();
    let vt0 = vt_cpu_ns();
    engine.play(secs(FROM_S), 1.0, 2).await.unwrap();
    tokio::time::sleep(Duration::from_secs_f64(PLAY_S)).await;
    let stopped = engine.pause().await;
    let vt_ms = (vt_cpu_ns() - vt0) as f64 / 1e6;
    let frames = stats
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .ms
        .len();
    let video_s = (stopped - secs(FROM_S)).abs() as f64 / 1e6;
    // The probe counts the 1-hour project's cuts, which this one-clip project doesn't have.
    stats
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .cuts
        .clear();
    report(
        &format!(
            "{name} ({:.1} frames/s drawn, {video_s:.1} s of video, VT CPU {:.0} ms per second of video)",
            frames as f64 / PLAY_S,
            vt_ms / video_s.max(0.001),
        ),
        &start,
        &stats,
        frames,
    );
    engine.shutdown().await;
}
