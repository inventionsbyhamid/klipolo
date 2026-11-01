//! `cargo run -p klipolo-engine --example perf_harness -- analysis <video to play> <clip analysed beside it>`: what
//! analysis's decoding beside playback costs it, and so what making way saves. The real actor and GPU backend play
//! `PLAY_S` of the video at 1x three times, each on a fresh engine: alone (what playback gets while the scene log's
//! picture decode makes way), beside the clip's every-frame picture decode (the scene log's: its cut finder reads each
//! frame grey, the decode half of its small copy), and beside the clip's sound decode (the speech copy's). The work
//! beside it runs on a thread of its own at utility priority, as analysis runs it, starting over at the clip's end.
//! Reported per run: frames drawn per second, render ms per frame, the late runs the engine said (`FramesLate`, at most
//! one a playback), and how far the work beside it got. Nothing is captured and no sound device opens. Cap the CPUs (a
//! container's `--cpus 4`) to see a small computer.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use klipolo_engine::api::{EngineEvent, ViewSpec};
use klipolo_engine::media::luma_frames;
use klipolo_engine::media::proxy::facts::probe_facts;
use klipolo_engine::platform::gpu::GpuClass;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::session::{Engine, EngineOptions, ProjectFeed};
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::timeline::{Clip, ClipSource};
use klipolo_platform::qos::{self, Qos};
use tokio::sync::{mpsc, watch};

use super::measure::{mark, report};
use super::project::secs;
use super::{Probe, Renders};

/// Seconds each run plays.
const PLAY_S: f64 = 8.0;
/// Where each run starts playing (s).
const FROM_S: f64 = 1.0;
/// The grey pictures' size: the cut finder's.
const GREY: (u32, u32) = (320, 180);

/// What runs beside playback.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Beside {
    Nothing,
    Picture,
    Sound,
}

pub async fn run(args: &[String]) {
    let (Some(video), Some(clip)) = (args.first(), args.get(1)) else {
        eprintln!("usage: perf_harness analysis <video to play> <clip analysed beside it>");
        return;
    };
    let video = std::fs::canonicalize(video)
        .expect("the video")
        .display()
        .to_string();
    let clip = std::fs::canonicalize(clip)
        .expect("the clip")
        .display()
        .to_string();
    let facts = probe_facts(Path::new(&video)).expect("a video the probe reads");
    println!(
        "analysis beside playback: playing {video} ({}x{} {} at {:.3} fps); analysing {clip}; {} cores here",
        facts.width,
        facts.height,
        facts.codec,
        facts.rate.f64(),
        std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
    );
    let project = Arc::new(project(
        &video,
        facts.width,
        facts.height,
        facts.rate.f64(),
        facts.duration_us,
    ));
    for (beside, name) in [
        (Beside::Nothing, "alone (the picture decode makes way)"),
        (
            Beside::Picture,
            "beside the clip's picture decode (it never made way)",
        ),
        (
            Beside::Sound,
            "beside the clip's sound decode (it goes on until frames come late)",
        ),
        (Beside::Nothing, "alone again"),
    ] {
        play(name, project.clone(), &clip, beside).await;
    }
}

/// `video` on the main track for its whole length.
fn project(video: &str, width: u32, height: u32, fps: f64, duration_us: i64) -> Project {
    let mut p = Project::new("Analysis");
    p.assets = vec![Asset {
        id: "video".into(),
        kind: AssetKind::Video,
        path: video.into(),
        name: "video".into(),
        duration_us,
        width,
        height,
        fps,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    }];
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset { id: "video".into() },
        0,
        duration_us,
    )];
    p
}

/// Decodes `clip` (`beside`) over and over on a thread of utility priority until `stop`; counts what it got through:
/// frames for the picture, blocks for the sound.
fn work_beside(
    clip: String,
    beside: Beside,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicU64>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        qos::set_current(Qos::Utility);
        while !stop.load(Ordering::Relaxed) {
            match beside {
                Beside::Nothing => return,
                Beside::Picture => {
                    let _ = luma_frames::each_frame(Path::new(&clip), GREY, &stop, &mut |_, _| {
                        done.fetch_add(1, Ordering::Relaxed);
                        true
                    });
                }
                Beside::Sound => {
                    let _ = klipolo_engine::audio::decode::decode(
                        Path::new(&clip),
                        0,
                        None,
                        &mut |_, _| {
                            done.fetch_add(1, Ordering::Relaxed);
                            Ok(!stop.load(Ordering::Relaxed))
                        },
                    );
                }
            }
        }
    })
}

/// A fresh engine on the GPU backend plays `project` for `PLAY_S` from `FROM_S`, with `beside` decoding `clip` on a
/// thread of its own.
async fn play(name: &str, project: Arc<Project>, clip: &str, beside: Beside) {
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
    let late = Arc::new(AtomicU64::new(0));
    {
        let late = late.clone();
        engine.set_event_sink(Arc::new(move |event| {
            if matches!(event, EngineEvent::FramesLate { .. }) {
                late.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }
    engine.set_view(ViewSpec {
        format: FormatKind::Landscape,
        width: 1280,
        height: 720,
    });
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
    let (stop, worked) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicU64::new(0)),
    );
    let side = work_beside(clip.to_owned(), beside, stop.clone(), worked.clone());
    // The work beside it under way first, as when the person presses play while a clip is analysed.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = worked.load(Ordering::Relaxed);
    let start = mark();
    let started = Instant::now();
    engine.play(secs(FROM_S), 1.0, 2).await.unwrap();
    tokio::time::sleep(Duration::from_secs_f64(PLAY_S)).await;
    let stopped = engine.pause().await;
    let wall = started.elapsed().as_secs_f64();
    let side_done = worked.load(Ordering::Relaxed) - before;
    stop.store(true, Ordering::Relaxed);
    let _ = side.join();
    let frames = stats
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .ms
        .len();
    let video_s = (stopped - secs(FROM_S)).abs() as f64 / 1e6;
    stats
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .cuts
        .clear();
    let unit = match beside {
        Beside::Nothing => "",
        Beside::Picture => " frames",
        Beside::Sound => " sound blocks",
    };
    report(
        &format!(
            "{name} ({:.1} frames/s drawn, {video_s:.1} s of video in {wall:.1} s, late runs {}, beside it {:.0}{unit}/s)",
            frames as f64 / PLAY_S,
            late.load(Ordering::Relaxed),
            side_done as f64 / wall,
        ),
        &start,
        &stats,
        frames,
    );
    engine.shutdown().await;
}
