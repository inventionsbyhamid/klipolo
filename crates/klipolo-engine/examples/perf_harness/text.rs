//! `cargo run -p klipolo-engine --example perf_harness --release -- text [1080|4k] [--seconds <s>] [--runs <n>]`: what
//! titles and captions cost (a caption track and a title entrance), through the GPU backend and
//! paced like playback, 60 frames a second on the host clock, so the caption and title build threads work while
//! frames wait.
//!
//! - Captions: `--seconds` (30) of speech, twelve words a second in six-word sentences, in four
//!   per-word styles: Highlight (the box glides to the word said; outline, shadow), Arcade (the word said glows;
//!   outline, the keyword "gaming"), Karaoke (the word said in its colour; outline, shadow) and Word pop (the word said
//!   grows). Portrait.
//! - Titles: three 30-character titles, plain and with a 4 px outline, entering in turn at 1, 2.5 and 4 s, a second
//!   each. First with the playback lookahead's warm-up (`PreviewBackend::warm` with the titles of the plan at each
//!   start, a second ahead, as `warm_ahead` calls it), then without it (a cold seek's path). Landscape.
//!
//! `1080` draws 1080 × 1920 captions and 1920 × 1080 titles; `4k` twice that each way. Per case: frame CPU ms (the
//! plan handed in until it is submitted) and GPU ms (submitted until its NV12 pixels are back), p50/p95/max; frames
//! over the 60 fps budget; caption frames drawn before their page's atlas was there; each page build's CPU and GPU ms
//! (the build thread waits for the GPU here, never the render thread); the atlases' and title textures' bytes; and for
//! titles, each entrance frame's times, what the render thread built as they entered, and the title build thread's
//! builds (CPU and GPU ms). Those GPU figures are wall times, the base video and the whole queue included.
//!
//! The drawer's own time: the title and caption drawers' CPU on the render thread each frame, and
//! their GPU time from timestamp queries (a title built on the render thread, and every title and caption drawn in the
//! frame's passes; "n/a" on an adapter without timestamps inside passes), p50/p95/max over the frames that draw text,
//! next to the text budgets (Normal p95 ≤ 2 ms GPU, ≤ 300 µs CPU; Light ≤ 0.5 ms, ≤ 100 µs). `--runs N` runs each
//! case N times, each on a backend of its own, and prints every figure's median over the runs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_engine::plan::{FramePlan, PlanContext, frame_plan};
use klipolo_engine::present::Output;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::render::captions::{build_times, time_builds};
use klipolo_engine::render::text as titles;
use klipolo_engine::render::text_timing::{TextTime, ask_for_timestamps};
use klipolo_engine::session::{PreviewBackend, RenderRequest};
use klipolo_model::captions::captions;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::project::{BackgroundKind, FormatKind, Project};
use klipolo_model::timeline::{Item, ItemContent, Outline, TextStyle, Track, TrackKind};

use super::measure::mb;
use super::project::secs;
use super::text_stats::{Spread, drawer, median, median_opt};

/// The preview's pace.
const FPS: f64 = 60.0;
/// A frame over this misses its slot at 60 fps.
const BUDGET_MS: f64 = 1000.0 / FPS;
/// Six-word sentences, twelve words a second, the last word the sentence's number; "gaming" is Arcade's keyword.
const WORDS: [&str; 6] = ["make", "this", "gaming", "clip", "pop", "today"];
const WORDS_PER_SECOND: f64 = 12.0;
/// 30 characters.
const TITLE: &str = "Thirty characters in one title";
/// The titles' starts (each on screen a second, its own text).
const ENTRANCES: [f64; 3] = [1.0, 2.5, 4.0];

/// One frame's times: the whole frame's walls, and its text drawers' own.
#[derive(Clone, Copy)]
struct Timed {
    cpu_ms: f64,
    gpu_ms: f64,
    text: TextTime,
}

/// Renders `plan` at `size` as playback does (NV12 back over the socket): its CPU and GPU times.
async fn frame(backend: &mut GpuBackend, plan: FramePlan, size: (u32, u32)) -> Timed {
    let started = Instant::now();
    let rendered = backend
        .render(RenderRequest {
            plan,
            width: size.0,
            height: size.1,
            output: Output::Nv12,
        })
        .await
        .expect("a frame");
    let submitted = Instant::now();
    let mut pixels = rendered.pixels;
    pixels.wait().await.expect("its pixels");
    let gpu_ms = submitted.elapsed().as_secs_f64() * 1e3;
    Timed {
        cpu_ms: (submitted - started).as_secs_f64() * 1e3,
        gpu_ms,
        text: backend.text_time(),
    }
}

/// The drawer's own spreads over the frames that drew text: CPU µs, and GPU ms unless the device has no timestamps.
fn drawer_spreads(frames: &[TextTime]) -> (Spread, Option<Spread>) {
    let drew: Vec<&TextTime> = frames.iter().filter(|t| t.drew).collect();
    let cpu = Spread::of(&drew.iter().map(|t| t.cpu_us).collect::<Vec<_>>());
    let gpu: Option<Vec<f64>> = drew.iter().map(|t| t.gpu_ms).collect();
    (cpu, gpu.map(|g| Spread::of(&g)))
}

/// Sleeps until frame `k`'s slot after `start` (no sleep when behind).
async fn pace(start: Instant, k: usize) {
    let at = start + Duration::from_secs_f64(k as f64 / FPS);
    tokio::time::sleep_until(at.into()).await;
}

pub async fn run(args: &[String]) {
    klipolo_engine::render::fonts::configure_for_tests();
    // Every backend below asks for GPU timestamps (the drawer's own GPU time) where the adapter has them.
    ask_for_timestamps();
    let k = if args.iter().any(|a| a.eq_ignore_ascii_case("4k")) {
        2
    } else {
        1
    };
    let number = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse::<f64>().ok())
    };
    let seconds = number("--seconds").unwrap_or(30.0).clamp(2.0, 55.0);
    let runs = number("--runs").unwrap_or(1.0).clamp(1.0, 9.0) as usize;
    let probe = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .expect("GPU");
    println!(
        "text at {} on {} (paced at {FPS} fps; GPU = submitted until the pixels are back; drawer = the text drawers' \
         own CPU and GPU timestamps{})",
        if k == 2 { "4K" } else { "1080p" },
        probe.adapter_name(),
        if runs > 1 {
            format!("; the median of {runs} runs")
        } else {
            String::new()
        }
    );
    drop(probe);
    for style in [
        "caption.highlight",
        "caption.arcade",
        "caption.karaoke",
        "caption.word-pop",
    ] {
        let mut all = Vec::new();
        for _ in 0..runs {
            all.push(captions_case(style, (1080 * k, 1920 * k), seconds).await);
        }
        CaptionRun::median(&all).print(style);
    }
    for outline in [false, true] {
        for warm in [true, false] {
            let mut all = Vec::new();
            for _ in 0..runs {
                all.push(title_case(outline, warm, (1920 * k, 1080 * k)).await);
            }
            TitleRun::median(&all).print(outline, warm);
        }
    }
}

/// One run of a captions case.
struct CaptionRun {
    pages: f64,
    page_rate: f64,
    frame_cpu: Spread,
    frame_gpu: Spread,
    over: f64,
    frames: f64,
    late: f64,
    builds: f64,
    build_cpu: Spread,
    build_gpu: Spread,
    atlases: f64,
    atlas_mb: f64,
    /// The drawer's uploads of its frame buffers per frame (only when they changed), and the frames that made
    /// them through a staging buffer of their own (every kept one in flight).
    uploads: f64,
    one_off_uploads: f64,
    drawer_cpu: Spread,
    drawer_gpu: Option<Spread>,
}

impl CaptionRun {
    fn median(runs: &[Self]) -> Self {
        let m = |f: fn(&Self) -> f64| median(runs.iter().map(f));
        let s = |f: fn(&Self) -> Spread| Spread::median(runs.iter().map(f));
        Self {
            pages: m(|r| r.pages),
            page_rate: m(|r| r.page_rate),
            frame_cpu: s(|r| r.frame_cpu),
            frame_gpu: s(|r| r.frame_gpu),
            over: m(|r| r.over),
            frames: m(|r| r.frames),
            late: m(|r| r.late),
            builds: m(|r| r.builds),
            build_cpu: s(|r| r.build_cpu),
            build_gpu: s(|r| r.build_gpu),
            atlases: m(|r| r.atlases),
            atlas_mb: m(|r| r.atlas_mb),
            uploads: m(|r| r.uploads),
            one_off_uploads: m(|r| r.one_off_uploads),
            drawer_cpu: s(|r| r.drawer_cpu),
            drawer_gpu: median_opt(runs.iter().map(|r| r.drawer_gpu), Spread::median),
        }
    }

    fn print(&self, style: &str) {
        println!(
            "  {style:<18} {} pages ({:.1} a second) | frame CPU p50/p95/max {} ms | GPU {} ms | over budget {}/{} | \
             late caption frames {} | page builds {}: CPU {} ms, GPU {} ms | atlases {} ({:.1} MB) | uploads {:.2} a frame ({:.2} one-off) | {}",
            self.pages,
            self.page_rate,
            self.frame_cpu.show(2),
            self.frame_gpu.show(2),
            self.over,
            self.frames,
            self.late,
            self.builds,
            self.build_cpu.show(2),
            self.build_gpu.show(2),
            self.atlases,
            self.atlas_mb,
            self.uploads,
            self.one_off_uploads,
            drawer(self.drawer_cpu, self.drawer_gpu),
        );
    }
}

/// The captions case: `style`'s pages over `seconds` of speech, drawn at `size`.
async fn captions_case(style: &str, size: (u32, u32), seconds: f64) -> CaptionRun {
    let count = (seconds * WORDS_PER_SECOND) as usize;
    let owned: Vec<(String, f64, f64)> = (0..count)
        .map(|i| {
            let at = i as f64 / WORDS_PER_SECOND;
            // Each sentence ends with its number, so no two pages are alike (atlases are kept by content).
            let mut word = WORDS[i % WORDS.len()].to_owned();
            if i % WORDS.len() == WORDS.len() - 1 {
                word = format!("n{}.", i / WORDS.len());
            }
            (word, at, at + 0.9 / WORDS_PER_SECOND)
        })
        .collect();
    let words: Vec<(&str, f64, f64)> = owned.iter().map(|(w, a, b)| (w.as_str(), *a, *b)).collect();
    let (p, speech) = klipolo_engine::test_support::sound_only_captions_project(style, &words);
    let pages = Arc::new(captions(&p, &speech));
    let page_count = pages
        .pages
        .get(&FormatKind::Portrait)
        .map_or(0, |p| p.pages.len());
    let ctx = PlanContext::new(Arc::new(p)).with_captions(pages);
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .expect("GPU");
    backend.warm_all();
    // The first page and the pipelines, as an opened project has them.
    frame(
        &mut backend,
        frame_plan(&ctx, 0, FormatKind::Portrait),
        size,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = build_times();
    time_builds(true);
    backend.time_text(true);
    let (_, before) = backend.text_stats();
    let frames = (seconds * FPS) as usize;
    let (mut cpu, mut gpu, mut text, mut over) = (Vec::new(), Vec::new(), Vec::new(), 0usize);
    let start = Instant::now();
    for n in 0..frames {
        pace(start, n).await;
        let plan = frame_plan(&ctx, secs(n as f64 / FPS), FormatKind::Portrait);
        let t = frame(&mut backend, plan, size).await;
        over += usize::from(t.cpu_ms + t.gpu_ms > BUDGET_MS);
        cpu.push(t.cpu_ms);
        gpu.push(t.gpu_ms);
        text.push(t.text);
    }
    time_builds(false);
    backend.time_text(false);
    let builds = build_times();
    let (_, after) = backend.text_stats();
    let (drawer_cpu, drawer_gpu) = drawer_spreads(&text);
    CaptionRun {
        pages: page_count as f64,
        page_rate: page_count as f64 / seconds,
        frame_cpu: Spread::of(&cpu),
        frame_gpu: Spread::of(&gpu),
        over: over as f64,
        frames: frames as f64,
        late: (after.late - before.late) as f64,
        builds: builds.len() as f64,
        build_cpu: Spread::of(&builds.iter().map(|b| b.cpu_ms).collect::<Vec<_>>()),
        build_gpu: Spread::of(&builds.iter().map(|b| b.gpu_ms).collect::<Vec<_>>()),
        atlases: after.atlases as f64,
        atlas_mb: mb(after.atlas_bytes),
        uploads: (after.uploads - before.uploads) as f64 / frames as f64,
        one_off_uploads: (after.one_off_uploads - before.one_off_uploads) as f64 / frames as f64,
        drawer_cpu,
        drawer_gpu,
    }
}

/// A project with three 30-character titles entering in turn (`ENTRANCES`), each a second, with a 4 px outline when
/// `outline`.
fn title_project(outline: bool) -> Project {
    let mut p = Project::new("Titles");
    p.timeline.main.clear();
    p.background.kind = BackgroundKind::Solid;
    p.background.color = "#202020".into();
    let mut track = Track::new(TrackKind::Overlay, "Titles");
    track.items = ENTRANCES
        .iter()
        .enumerate()
        .map(|(k, &at)| Item {
            id: format!("title-{k}"),
            start_us: secs(at),
            duration_us: secs(1.0),
            attach: None,
            layout: Layout::new(
                Anchor::TopLeft,
                BoxN {
                    x: 0.1,
                    y: 0.4,
                    w: 0.8,
                    h: 0.2,
                },
            ),
            content: ItemContent::Text {
                text: format!("{}{k}", &TITLE[..TITLE.len() - 1]),
                style: TextStyle {
                    size: 96.0,
                    outline: outline.then(|| Outline {
                        color: "#000000".into(),
                        width: 4.0,
                    }),
                    ..TextStyle::default()
                },
            },
            effects: Vec::new(),
        })
        .collect();
    p.timeline.tracks = vec![track];
    p
}

/// One run of a title case. Entrance frames: whole-frame CPU and GPU ms, the drawer's CPU µs and GPU ms (NaN: n/a).
struct TitleRun {
    entrances: Vec<[f64; 4]>,
    built: f64,
    ahead: f64,
    ahead_cpu: Spread,
    ahead_gpu: Spread,
    frame_cpu: Spread,
    frame_gpu: Spread,
    over: f64,
    frames: f64,
    texture_mb: f64,
    drawer_cpu: Spread,
    drawer_gpu: Option<Spread>,
}

impl TitleRun {
    fn median(runs: &[Self]) -> Self {
        let m = |f: fn(&Self) -> f64| median(runs.iter().map(f));
        let s = |f: fn(&Self) -> Spread| Spread::median(runs.iter().map(f));
        let entrances = (0..runs.iter().map(|r| r.entrances.len()).min().unwrap_or(0))
            .map(|e| std::array::from_fn(|i| median(runs.iter().map(|r| r.entrances[e][i]))))
            .collect();
        Self {
            entrances,
            built: m(|r| r.built),
            ahead: m(|r| r.ahead),
            ahead_cpu: s(|r| r.ahead_cpu),
            ahead_gpu: s(|r| r.ahead_gpu),
            frame_cpu: s(|r| r.frame_cpu),
            frame_gpu: s(|r| r.frame_gpu),
            over: m(|r| r.over),
            frames: m(|r| r.frames),
            texture_mb: m(|r| r.texture_mb),
            drawer_cpu: s(|r| r.drawer_cpu),
            drawer_gpu: median_opt(runs.iter().map(|r| r.drawer_gpu), Spread::median),
        }
    }

    fn print(&self, outline: bool, warm: bool) {
        let firsts: Vec<String> = self
            .entrances
            .iter()
            .map(|[cpu, gpu, own_cpu, own_gpu]| {
                let own_gpu = if own_gpu.is_nan() {
                    "n/a".to_owned()
                } else {
                    format!("{own_gpu:.2}")
                };
                format!("{cpu:.2}/{gpu:.2} (drawer {own_cpu:.0} µs/{own_gpu})")
            })
            .collect();
        println!(
            "  titles{} {} | entrance frames CPU/GPU ms {} | built on the render thread as they entered: {} | built \
             ahead {}: CPU {} ms, GPU {} ms | frame CPU p50/p95/max {} ms | GPU {} ms | over budget {}/{} | title \
             textures at most {:.1} MB | {}",
            if outline {
                ", 4 px outline"
            } else {
                "                "
            },
            if warm { "(lookahead)" } else { "(cold)     " },
            firsts.join(", "),
            self.built,
            self.ahead,
            self.ahead_cpu.show(2),
            self.ahead_gpu.show(2),
            self.frame_cpu.show(2),
            self.frame_gpu.show(2),
            self.over,
            self.frames,
            self.texture_mb,
            drawer(self.drawer_cpu, self.drawer_gpu),
        );
    }
}

/// The title case: 5.5 s of frames at `size`, each title's start warmed a second ahead when `warm`.
async fn title_case(outline: bool, warm: bool, size: (u32, u32)) -> TitleRun {
    let ctx = PlanContext::new(Arc::new(title_project(outline)));
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .expect("GPU");
    backend.warm_all();
    frame(
        &mut backend,
        frame_plan(&ctx, 0, FormatKind::Landscape),
        size,
    )
    .await;
    let _ = titles::build_times();
    titles::time_builds(true);
    backend.time_text(true);
    let frames = (5.5 * FPS) as usize;
    let at = |t: f64| (t * FPS).round() as usize;
    let (mut cpu, mut gpu, mut text, mut over) = (Vec::new(), Vec::new(), Vec::new(), 0usize);
    let (mut entering, mut built, mut bytes) = (Vec::new(), 0u64, 0u64);
    let start = Instant::now();
    for n in 0..frames {
        pace(start, n).await;
        if warm && let Some(&s) = ENTRANCES.iter().find(|&&s| at(s - 1.0) == n) {
            // `warm_ahead`: the lookahead reaches each title's start a second before it, and warms its titles alone
            // (what the app warms, no more).
            backend.warm(&frame_plan(&ctx, secs(s), FormatKind::Landscape).titles_only());
        }
        let (before, _) = backend.text_stats();
        let plan = frame_plan(&ctx, secs(n as f64 / FPS), FormatKind::Landscape);
        let t = frame(&mut backend, plan, size).await;
        let (after, _) = backend.text_stats();
        if ENTRANCES.iter().any(|&s| (at(s)..at(s) + 3).contains(&n)) {
            built += after.built_here - before.built_here;
        }
        if ENTRANCES.iter().any(|&s| at(s) == n) {
            entering.push([
                t.cpu_ms,
                t.gpu_ms,
                t.text.cpu_us,
                t.text.gpu_ms.unwrap_or(f64::NAN),
            ]);
        }
        bytes = bytes.max(after.bytes);
        over += usize::from(t.cpu_ms + t.gpu_ms > BUDGET_MS);
        cpu.push(t.cpu_ms);
        gpu.push(t.gpu_ms);
        text.push(t.text);
    }
    titles::time_builds(false);
    backend.time_text(false);
    let ahead = titles::build_times();
    let (drawer_cpu, drawer_gpu) = drawer_spreads(&text);
    TitleRun {
        entrances: entering,
        built: built as f64,
        ahead: ahead.len() as f64,
        ahead_cpu: Spread::of(&ahead.iter().map(|b| b.cpu_ms).collect::<Vec<_>>()),
        ahead_gpu: Spread::of(&ahead.iter().map(|b| b.gpu_ms).collect::<Vec<_>>()),
        frame_cpu: Spread::of(&cpu),
        frame_gpu: Spread::of(&gpu),
        over: over as f64,
        frames: frames as f64,
        texture_mb: mb(bytes),
        drawer_cpu,
        drawer_gpu,
    }
}
