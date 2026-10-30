//! `cargo run -p klipolo-engine --example perf_harness -- reverse [<1080p file>]`: what shuttle playback
//! costs on a 1080p screen recording (default the 60 s 1080p30 fixture, a keyframe every 0.5 s: `gen-engine-fixtures.sh
//! --bench`). Nothing is captured and no sound device opens.
//!
//! 1. Decode: the preview's decoder pool shows every frame of 5 s of video, one request at a time as playback asks,
//!    forward and then backward. Reported per second of video: the wall time the requests took back to back (under
//!    1000 ms keeps up with 1× playback), the VideoToolbox decoder service's CPU time, and the requests that waited
//!    for a reader to move (over 20 ms: backward, each one decodes from a keyframe up to the frame asked for). Then
//!    the same with the FFmpeg decoder.
//! 2. Playback: the real actor and GPU backend play 4 s at 1×, −1×, 2× and −2× (as the harness's playback does: pixels
//!    read back and waited for): frames drawn per second and render ms per frame.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use klipolo_engine::api::ViewSpec;
use klipolo_engine::plan::framing::FULL;
use klipolo_engine::plan::{
    BackgroundPlan, FramePlan, Layer, SourceRef, VideoLayer, VideoRole, frame_start_us,
};
use klipolo_engine::render::pool::{DecodedSet, DecoderPool};
use klipolo_engine::render::{GpuBackend, PREVIEW_TUNING};
use klipolo_engine::session::{Engine, EngineOptions, ProjectFeed};
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Clip, ClipSource};
use tokio::sync::{mpsc, watch};

use super::measure::{mark, pct, report, vt_cpu_ns};
use super::project::{fixture, secs};
use super::{Probe, Renders};

const FPS: u32 = 30;
/// Frames of video each decode run shows (5 s).
const FRAMES: i64 = 150;
/// A request this slow waited for its reader to move (a reset and a decode from the keyframe).
const SLOW_MS: f64 = 20.0;

pub async fn run(args: &[String]) {
    let path = args
        .first()
        .cloned()
        .unwrap_or_else(|| fixture("screen-60s-1080p30.mp4"));
    println!("reverse playback on {path}");
    // Forward over 10–15 s, backward over 35–30 s: apart, so neither run finds the other's frames. Then the FFmpeg
    // decoder (the fallback for files AVFoundation can't read, and Windows and Linux) the same way.
    for (ffmpeg, decoder) in [(false, "AVAssetReader"), (true, "FFmpeg")] {
        let runs = [
            ("forward 1×", (300..300 + FRAMES).collect::<Vec<_>>()),
            ("backward −1×", (900 - FRAMES..900).rev().collect()),
            (
                "backward −2×",
                (1200 - 2 * FRAMES..1200).rev().step_by(2).collect(),
            ),
        ];
        for (name, frames) in runs {
            decode(&format!("decode {name} ({decoder})"), &path, frames, ffmpeg).await;
        }
    }
    playback(&path).await;
}

/// The main footage alone at source time `time_us`.
fn plan_at(path: &str, time_us: Us) -> FramePlan {
    FramePlan {
        format: FormatKind::Landscape,
        canvas: (1920, 1080),
        at_us: time_us,
        background: BackgroundPlan::Solid { color: [0; 3] },
        layers: vec![Layer::Video(VideoLayer {
            role: VideoRole::Main,
            id: "screen".into(),
            source: SourceRef {
                asset_id: "screen".into(),
                path: path.into(),
                original: path.into(),
                time_us,
                fps: f64::from(FPS),
                rotation: None,
                width: 1920,
                height: 1080,
            },
            dest: FULL,
            crop: FULL,
            rotation: 0.0,
            opacity: 1.0,
            radius: 0.0,
            mirror: false,
            border: false,
            look: None,
            effects: Vec::new(),
            pad_px: 0.0,
            blend: klipolo_model::motion::Blend::Normal,
            reach: 1.0,
        })],
        missing: Vec::new(),
    }
}

/// Shows `frames` in order through a fresh preview decoder pool, timing each request.
async fn decode(name: &str, path: &str, frames: Vec<i64>, force_ffmpeg: bool) {
    let mut pool = DecoderPool::new(PREVIEW_TUNING, force_ffmpeg);
    let mut set = DecodedSet::default();
    // Open the reader where the run starts (the playback lookahead does this ahead of time).
    pool.decode(&plan_at(path, frame_start_us(frames[0], FPS)), &mut set)
        .await;
    let (vt0, wall) = (vt_cpu_ns(), Instant::now());
    let mut ms = Vec::with_capacity(frames.len());
    for &n in &frames[1..] {
        let started = Instant::now();
        pool.decode(&plan_at(path, frame_start_us(n, FPS)), &mut set)
            .await;
        ms.push(started.elapsed().as_secs_f64() * 1000.0);
        assert!(set.get(0).is_some(), "frame {n} wasn't decoded");
    }
    let video_s = ((frames[0] - frames[frames.len() - 1]).abs() as f64) / f64::from(FPS);
    let wall_ms = wall.elapsed().as_secs_f64() * 1000.0;
    let vt_ms = (vt_cpu_ns() - vt0) as f64 / 1e6;
    let slow = ms.iter().filter(|&&m| m > SLOW_MS).count();
    ms.sort_by(f64::total_cmp);
    println!(
        "{name}: {} frames over {video_s:.1} s of video | per second of video: {:.0} ms wall, VT CPU {:.0} ms | \
         request ms p50 {:.2} p95 {:.1} max {:.1} | {slow} waited for a reader (> {SLOW_MS} ms)",
        ms.len(),
        wall_ms / video_s,
        vt_ms / video_s,
        pct(&ms, 0.5),
        pct(&ms, 0.95),
        ms.last().copied().unwrap_or(0.0),
    );
}

/// One 60 s clip of `path`.
fn project(path: &str) -> Project {
    let mut p = Project::new("Reverse");
    p.assets = vec![Asset {
        id: "screen".into(),
        kind: AssetKind::Video,
        path: path.into(),
        name: "screen".into(),
        duration_us: secs(60.0),
        width: 1920,
        height: 1080,
        fps: f64::from(FPS),
        has_audio: false,
        audio_lead_us: None,
        rotation: None,
    }];
    p.timeline.main = vec![Clip::new(
        ClipSource::Asset {
            id: "screen".into(),
        },
        0,
        secs(60.0),
    )];
    p
}

/// The actor and GPU backend playing 4 s at each shuttle rate.
async fn playback(path: &str) {
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
            gpu_class: klipolo_engine::platform::gpu::GpuClass::Integrated,
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
    let project = Arc::new(project(path));
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
    for (run, (rate, from)) in [(1.0, 10.0), (-1.0, 40.0), (2.0, 15.0), (-2.0, 55.0)]
        .into_iter()
        .enumerate()
    {
        // The still at the start first, so each run starts with its decoder there (as after a click).
        let seq = run as u32 + 1;
        engine.seek(secs(from), seq);
        tokio::time::sleep(Duration::from_millis(500)).await;
        while done.try_recv().is_ok() {}
        *stats.lock().unwrap_or_else(PoisonError::into_inner) = Renders::default();
        let start = mark();
        let vt0 = vt_cpu_ns();
        engine.play(secs(from), rate, seq).await.unwrap();
        tokio::time::sleep(Duration::from_secs(4)).await;
        let stopped = engine.pause().await;
        let vt_ms = (vt_cpu_ns() - vt0) as f64 / 1e6;
        let frames = stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .ms
            .len();
        let video_s = (stopped - secs(from)).abs() as f64 / 1e6;
        // The probe counts the 1-hour project's cuts, which this one-clip project doesn't have.
        stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cuts
            .clear();
        report(
            &format!(
                "playback {rate}× (4 s, {:.1} frames/s drawn, {video_s:.1} s of video, VT CPU {:.0} ms per second of video)",
                frames as f64 / 4.0,
                vt_ms / video_s.max(0.001),
            ),
            &start,
            &stats,
            frames,
        );
    }
    engine.shutdown().await;
}
