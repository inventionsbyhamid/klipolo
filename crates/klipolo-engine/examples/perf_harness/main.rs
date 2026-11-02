//! Headless engine perf harness: a synthetic 1-hour project with 300 layers, driven
//! through the real engine actor and GPU backend. Sound is never opened (the wrapper backend drops every audio call),
//! and nothing is captured. Media comes from `scripts/gen-engine-fixtures.sh`.
//!
//! `cargo run -p klipolo-engine --example perf_harness [--release]`; `… -- effects` runs only the effect-stage scenario
//! (`effects.rs`); `… -- motion` what layer motion, blend modes and scenes cost (`motion.rs`, `scenes.rs`); `… -- text`
//! `[1080|4k]` what titles and captions cost, paced like playback (`text.rs`); `… -- warm [--home <ms>]` times a first
//! open with and without the GPU warm-up (`warm.rs`);
//! `… -- export [<worker binary>]` runs the export worker scenario (`export.rs`); `… -- fontstart [<faces>]` what an
//! export worker's fonts cost before frame 0, scanned or from its job's record (`fontstart.rs`);
//! `… -- reverse [<file>]` what shuttle playback costs, backward and at 2× (`reverse.rs`); `… -- copy <file> [<copy>]`
//! what an editing copy saves while the preview plays, with and without a picture-in-picture layer (`copy.rs`);
//! `… -- analysis <video> <clip>` what analysis's decoding of a clip costs playback beside it (`analysis.rs`);
//! `… -- long [<video>]` what a 2-hour talk's project costs to save, open and edit, and the engine's work per edit
//! (`long.rs`); `… -- media <job> <file>...` what one media job costs a file, alone in the process (`media.rs`);
//! `… -- layers [1080|4k] <main> <video>...` what four cropped, moving video layers over a blurred background cost
//! (`layers.rs`);
//! `… -- --export-worker <job.json>` is the export worker itself (as the app runs it), and `… -- --proxy-worker
//! <proxy.json>` the editing-copy worker.
//!
//! Reports per scenario: wall ms per frame (p50/p95/max), Rust heap allocations and bytes per frame (whole process,
//! so the actor's planning and publishing count too), process CPU ms per frame, our peak `phys_footprint`, and the
//! footprint of the VideoToolbox decoder services (VT decodes out of process).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use klipolo_engine::api::{EngineEvent, ViewSpec};
use klipolo_engine::audio::plan::AudioPlan;
use klipolo_engine::plan::{FramePlan, PlanContext, frame_plan};
use klipolo_engine::render::GpuBackend;
use klipolo_engine::session::{
    Engine, EngineOptions, Pixels, PreviewBackend, ProjectFeed, RenderRequest, Rendered,
};
use klipolo_model::project::FormatKind;
use klipolo_model::time::Us;
use tokio::sync::{mpsc, watch};

mod analysis;
mod copy;
mod effects;
mod export;
#[path = "../../tests/support/fixture_set.rs"]
mod fixture_set;
mod fontstart;
mod layers;
mod long;
mod measure;
mod media;
mod motion;
mod project;
mod reverse;
mod scenes;
mod text;
mod text_stats;
mod vt_memory;
mod warm;

use measure::{allocs, mark, mb, me, pct, report, vt_footprint};
use project::{one_hour_project, secs};

// ---------------------------------------------------------------------------------------------------------------
// The wrapper backend: times every render and never opens a sound device.

#[derive(Default)]
struct Renders {
    ms: Vec<f64>,
    /// Render ms of frames that are the first of a main clip (a cut).
    cuts: Vec<f64>,
    backend_allocs: u64,
}

struct Probe<B> {
    inner: B,
    stats: Arc<Mutex<Renders>>,
    done: mpsc::UnboundedSender<Us>,
}

impl<B: PreviewBackend> PreviewBackend for Probe<B> {
    /// Times submit → pixels in memory (the probe waits for them, then hands the actor a ready frame).
    async fn render(&mut self, req: RenderRequest) -> Result<Rendered, String> {
        let at = req.plan.at_us;
        let (a0, _) = allocs();
        let started = Instant::now();
        let mut result = self.inner.render(req).await;
        if let Ok(r) = &mut result {
            match r.pixels.wait().await {
                Ok(bytes) => r.pixels = Pixels::Ready(Some(bytes)),
                Err(e) => result = Err(e),
            }
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        {
            let mut s = self.stats.lock().unwrap_or_else(PoisonError::into_inner);
            s.ms.push(ms);
            if project::is_cut(at) {
                s.cuts.push(ms);
            }
            s.backend_allocs += allocs().0 - a0;
        }
        if let Err(e) = &result {
            eprintln!("render failed: {e}");
        }
        let _ = self.done.send(at);
        result
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn release(&mut self) {
        self.inner.release();
    }

    fn warm(&mut self, plan: &FramePlan) {
        self.inner.warm(plan);
    }

    // Sound: never opened by the harness (the picture runs on the host clock).
    fn start_audio(&mut self, _plan: Arc<AudioPlan>, _from_us: Us) -> bool {
        false
    }
    fn update_audio(&mut self, _plan: Arc<AudioPlan>, _at_us: Us) {}
    fn stop_audio(&mut self) {}
    fn prepare_audio(&mut self, _plan: Arc<AudioPlan>, _at_us: Us) {}
}

fn main() {
    // The bundled fonts from the repository: a release build has no other folder to find them in (`bundled_dir`), and
    // its export worker, this binary again, inherits the variable.
    if std::env::var_os("KLIPOLO_FONTS_DIR").is_none() {
        // SAFETY: the first thing `main` does, before any other thread exists.
        unsafe {
            std::env::set_var(
                "KLIPOLO_FONTS_DIR",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts"),
            )
        };
    }
    // The export worker, as the app runs it (`--export-worker <job.json>`): tests and `export` use this binary.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--export-worker") {
        std::process::exit(klipolo_engine::export::worker::main(&args[1..]));
    }
    // The editing-copy worker, likewise (`--proxy-worker <proxy.json>`).
    if args.first().map(String::as_str) == Some("--proxy-worker") {
        std::process::exit(klipolo_engine::media::proxy::worker::main(&args[1..]));
    }
    harness();
}

#[tokio::main(flavor = "multi_thread")]
async fn harness() {
    if std::env::args().any(|a| a == "effects") {
        effects::run().await;
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("motion") {
        scenes::motion().await;
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("text") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        text::run(&args).await;
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("warm") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        warm::run(&args).await;
        return;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("export") => return export::run(&args[1..]),
        Some("reverse") => return reverse::run(&args[1..]).await,
        Some("copy") => return copy::run(&args[1..]).await,
        Some("analysis") => return analysis::run(&args[1..]).await,
        Some("export-job") => return export::write_job(&args[1..]),
        Some("export-case") => return export::case(&args[1..]),
        Some("vt-memory") => return vt_memory::run(&args[1..]).await,
        Some("fontstart") => return fontstart::run(&args[1..]),
        Some("fontstart-child") => return fontstart::child(&args[1..]),
        Some("long") => return long::run(&args[1..]),
        Some("layers") => return layers::run(&args[1..]).await,
        Some("media") => return media::run(&args[1..]).await,
        _ => {}
    }
    let scratch = tempfile::tempdir().unwrap();
    let project = Arc::new(one_hour_project(scratch.path()));
    let vt_before = vt_footprint();
    println!(
        "1-hour project: {} clips, {} layers; VT XPC before: {:.0} MB; footprint at start {:.0} MB",
        project.timeline.main.len(),
        project
            .timeline
            .tracks
            .iter()
            .map(|t| t.items.len())
            .sum::<usize>(),
        mb(vt_before),
        mb(me().footprint),
    );

    // Planning (pure): the per-revision context (zoom springs) and one frame plan.
    let started = Instant::now();
    let ctx = PlanContext::new(project.clone());
    let ctx_ms = started.elapsed().as_secs_f64() * 1000.0;
    let (a0, b0) = allocs();
    let started = Instant::now();
    let plans = 2_000;
    for k in 0..plans {
        let t = secs(3_599.0 * k as f64 / plans as f64);
        std::hint::black_box(frame_plan(&ctx, t, FormatKind::Landscape));
    }
    let (a1, b1) = allocs();
    println!(
        "plan: PlanContext::new {ctx_ms:.1} ms | frame_plan {:.1} µs, {:.1} allocs, {:.2} KB each",
        started.elapsed().as_secs_f64() * 1e6 / plans as f64,
        (a1 - a0) as f64 / plans as f64,
        (b1 - b0) as f64 / plans as f64 / 1024.0,
    );
    drop(ctx);

    // The real actor with the GPU backend behind the probe.
    let stats = Arc::new(Mutex::new(Renders::default()));
    let (done_tx, mut done) = mpsc::unbounded_channel();
    let gpu = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .expect("GPU");
    println!(
        "adapter: {} (preview budget {} px)",
        gpu.adapter_name(),
        gpu.max_preview_pixels()
    );
    let backend = Probe {
        inner: gpu,
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
    let (events_tx, mut events) = mpsc::unbounded_channel();
    engine.set_event_sink(Arc::new(move |e: EngineEvent| {
        let _ = events_tx.send(e);
    }));
    engine.set_view(ViewSpec {
        format: FormatKind::Landscape,
        width: 1280,
        height: 720,
    });
    let open = Instant::now();
    feed_tx.send_replace(Some(ProjectFeed {
        project_id: project.id.clone(),
        revision: 0,
        opened: 1,
        project: project.clone(),
        speech: Arc::default(),
        text_language: "en".into(),
        registry: klipolo_engine::registry::Registry::builtin(),
    }));
    let wait = async |done: &mut mpsc::UnboundedReceiver<Us>, at: Option<Us>| {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match tokio::time::timeout_at(deadline, done.recv()).await {
                Ok(Some(t)) if at.is_none_or(|a| a == t) => return true,
                Ok(Some(_)) => {}
                _ => return false,
            }
        }
    };
    assert!(wait(&mut done, None).await, "first frame");
    println!(
        "open → first frame: {:.1} ms",
        open.elapsed().as_secs_f64() * 1000.0
    );
    *stats.lock().unwrap_or_else(PoisonError::into_inner) = Renders::default();

    // Scrub: 150 seeks over the hour (titles, images, b-roll, zooms and plain footage), each waited for.
    let mut times = Vec::new();
    for j in 0..30 {
        let base = 72.0 * j as f64;
        times.extend([
            base + 1.0,
            base + 7.0,
            base + 13.0,
            base + 21.0,
            base + 40.0,
        ]);
    }
    let start = mark();
    let mut latencies = Vec::new();
    for (seq, t) in times.iter().enumerate() {
        let at = secs(*t);
        let sent = Instant::now();
        engine.seek(at, seq as u32 + 1);
        if !wait(&mut done, Some(at)).await {
            eprintln!("seek to {t} s: no frame");
        }
        latencies.push(sent.elapsed().as_secs_f64() * 1000.0);
    }
    report("scrub (150 seeks)", &start, &stats, times.len());
    latencies.sort_by(f64::total_cmp);
    println!(
        "  seek → frame ms p50 {:.1} p95 {:.1} max {:.1}",
        pct(&latencies, 0.5),
        pct(&latencies, 0.95),
        latencies.last().copied().unwrap_or(0.0)
    );

    // Still re-render of the same frame (an edit or a resize at the same time): the steady state.
    let start = mark();
    let still = secs(72.0 * 10.0 + 7.5);
    for k in 0..60 {
        engine.seek(still + (k % 2), 1_000 + k as u32);
        wait(&mut done, Some(still + (k % 2))).await;
    }
    report("still (60 re-renders)", &start, &stats, 60);

    // Playback: 10 s from 725.5 s (an image, clip cuts every 2.4 s, a b-roll start at 732 s).
    let from = secs(72.0 * 10.0 + 5.5);
    let start = mark();
    // Newer than the scrub and still seeks above.
    engine.play(from, 1.0, 2_000).await.unwrap();
    tokio::time::sleep(Duration::from_secs(10)).await;
    let stopped = engine.pause().await;
    let frames = stats
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .ms
        .len();
    report(
        &format!(
            "playback (10 s, {:.1} fps)",
            frames as f64 / ((stopped - from) as f64 / 1e6)
        ),
        &start,
        &stats,
        frames,
    );
    while events.try_recv().is_ok() {}

    let end = me();
    println!(
        "peak footprint {:.0} MB (ours) + VT XPC now {:.0} MB (was {:.0} MB before)",
        mb(end.peak_footprint),
        mb(vt_footprint()),
        mb(vt_before)
    );
    engine.shutdown().await;
}
