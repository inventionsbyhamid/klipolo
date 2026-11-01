//! `… -- motion [scenes] [1080|4k]`: what motion graphics with text cost per frame: a kinetic title (letters rising;
//! words blurring in), a blur-in title (twelve letters blurring in one after another; a node blur on one unsplit
//! 40-character line) and three lower thirds in full-frame layers, as authored (plain; with a shadowed title and role;
//! inside a faded group), staggered so all three animate together and then all three hold. Each through the real GPU
//! backend (`Headless::render_timed`: the frame's GPU time from its timestamps where the device has trusted ones, else
//! the fenced wall clock), at 1080p and 4K. A timed frame draws every pass, so "holding" times the three lower thirds
//! drawn while they hold still, not the held pictures the preview shows then.
//!
//! Printed per case and size: GPU ms p50/p95 (and frames 0–15 alone for a blur-in: its peak), the frame's encoding
//! CPU and the scene drawer's CPU (its last scene's) p50/p95, the drawer's first frame and its timeline survey alone,
//! passes a frame, the letter blurs' texture fetches and the text bytes written a frame, the last scene's `SceneBytes`,
//! and the effect pool's peak (`effect_target_peak`). The title and caption cases are the `text` subcommand's.

use std::sync::Arc;

use klipolo_engine::headless::Headless;
use klipolo_engine::registry::{Registry, compile_scene};
use klipolo_engine::render::effects::{effect_target_peak, reset_effect_target_peak};
use klipolo_model::contrib::{Catalog, ContribDef, ContribKind};
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::project::{FormatKind, Project};
use klipolo_model::timeline::{Item, ItemContent, Track, TrackKind};

use super::measure::pct;
use super::project::secs;

#[path = "scene_cases.rs"]
mod cases;
use cases::{Case, cases};

/// `… -- motion [scenes] [1080|4k] [<words>]`: layer motion's costs (`motion.rs`), then the scenes' (only the
/// scenes' with `scenes`; only the cases whose names hold one of `words` when given).
pub async fn motion() {
    let args: Vec<String> = std::env::args().skip(2).collect();
    if args.first().map(String::as_str) != Some("scenes") {
        super::motion::run().await;
    }
    run(&args).await;
}

fn scene_id(name: &str) -> String {
    format!("klipolo.perf/scene.{name}")
}

/// The built-ins plus each `(name, scene JSON)`, checked and compiled as a verified scene is.
fn registry(scenes: &[(&str, &str)]) -> Arc<Registry> {
    let catalog = Catalog::builtin();
    let compiled = scenes.iter().map(|(name, json)| {
        let def = ContribDef::new(scene_id(name).as_str(), ContribKind::Scene, name);
        let scene = compile_scene(json.as_bytes(), catalog)
            .unwrap_or_else(|problems| panic!("{name}: {problems:#?}"));
        (def, scene)
    });
    Arc::new(Registry::builtin().with_scenes(compiled))
}

/// A project over black of `layers` full-frame overlay layers, each showing scene `name` from its start for 4 s.
fn project(name: &str, starts: &[f64]) -> Project {
    let mut p = Project::new("Scene costs");
    p.id = "scene-costs".into();
    for (k, &start) in starts.iter().enumerate() {
        let content = ItemContent::Generator {
            source: scene_id(name).as_str().into(),
            params: Default::default(),
            still: false,
            offset_us: 0,
            span_us: None,
            seed_from: None,
        };
        let mut track = Track::new(TrackKind::Overlay, format!("Layer {k}"));
        track.items = vec![Item {
            id: format!("layer-{k}"),
            start_us: secs(start),
            duration_us: secs(4.0),
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
            content,
            effects: Vec::new(),
        }];
        p.timeline.tracks.push(track);
    }
    p
}

/// What one phase of frames measured.
#[derive(Default)]
struct Phase {
    gpu: Vec<f64>,
    cpu: Vec<f64>,
    drawer: Vec<f64>,
    passes: Vec<u32>,
    fetches: Vec<f64>,
    written: Vec<f64>,
    timestamps: bool,
}

async fn run(args: &[String]) {
    let sizes: Vec<(&str, (u32, u32))> = [("1080p", (1920, 1080)), ("4K", (3840, 2160))]
        .into_iter()
        .filter(|(n, _)| {
            let only = args.iter().find(|a| *a == "1080" || *a == "4k");
            only.is_none_or(|o| (o == "1080") == (*n == "1080p"))
        })
        .collect();
    // Any other word picks the cases whose names hold it (`… -- motion scenes dot`).
    let only: Vec<&String> = args
        .iter()
        .filter(|a| !["scenes", "1080", "4k"].contains(&a.as_str()))
        .collect();
    println!("scene costs, each case on a fresh device");
    for case in cases() {
        if !only.is_empty() && !only.iter().any(|w| case.name.contains(w.as_str())) {
            continue;
        }
        for &(label, size) in &sizes {
            measure(&case, label, size).await;
        }
    }
}

async fn measure(case: &Case, label: &str, size: (u32, u32)) {
    let registry = registry(&[("s", &case.json)]);
    let p = Arc::new(project("s", case.starts));
    let mut h = Headless::new(registry).await.expect("GPU");
    reset_effect_target_peak();
    // The first frame: the text built (waited for), the instance surveyed.
    let first = h
        .render_timed(
            p.clone(),
            secs(case.phases[0].1[0]),
            FormatKind::Landscape,
            size,
        )
        .await
        .expect("frame");
    let first_stats = h.scene_stats();
    let mut results = Vec::new();
    for (phase, at) in &case.phases {
        let mut m = Phase::default();
        for &t in at {
            let f = h
                .render_timed(p.clone(), secs(t), FormatKind::Landscape, size)
                .await
                .expect("frame");
            m.gpu.push(f.gpu_ms);
            m.cpu.push(f.cpu_us);
            m.timestamps |= f.timestamps;
            let stats = h.scene_stats();
            m.drawer.push(stats.cpu_us_last);
            m.passes.push(h.passes_last_frame());
            m.fetches.push(stats.blur_fetches as f64);
            m.written.push(stats.text_written as f64);
        }
        results.push((*phase, m));
    }
    let stats = h.scene_stats();
    println!(
        "  {} at {label}: first frame GPU {:.2} ms, encode {:.0} µs, drawer {:.0} µs (its survey: {:.0} µs here, {} run; \
         {:.0} µs on the text build thread)",
        case.name,
        first.gpu_ms,
        first.cpu_us,
        first_stats.cpu_us_last,
        first_stats.survey_us,
        first_stats.surveys,
        // A scene without text: its survey lands after its first frame.
        first_stats.survey_built_us.max(stats.survey_built_us)
    );
    for (phase, m) in &mut results {
        let in_order = m.gpu.clone();
        m.gpu.sort_by(f64::total_cmp);
        m.cpu.sort_by(f64::total_cmp);
        m.drawer.sort_by(f64::total_cmp);
        m.passes.sort_unstable();
        m.fetches.sort_by(f64::total_cmp);
        m.written.sort_by(f64::total_cmp);
        println!(
            "    {phase:<12} GPU p50 {:7.2} p95 {:7.2} ms ({}) | encode CPU p50 {:6.0} p95 {:6.0} µs | drawer p50 {:6.0} \
             p95 {:6.0} µs | passes {}–{}",
            pct(&m.gpu, 0.5),
            pct(&m.gpu, 0.95),
            if m.timestamps { "timestamps" } else { "wall" },
            pct(&m.cpu, 0.5),
            pct(&m.cpu, 0.95),
            pct(&m.drawer, 0.5),
            pct(&m.drawer, 0.95),
            m.passes.first().copied().unwrap_or(0),
            m.passes.last().copied().unwrap_or(0),
        );
        println!(
            "      blur fetches a frame p50 {:.0} max {:.0} | text bytes written a frame p50 {:.0} max {:.0}",
            pct(&m.fetches, 0.5),
            pct(&m.fetches, 1.0),
            pct(&m.written, 0.5),
            pct(&m.written, 1.0)
        );
        if phase.starts_with("frames 0") {
            let each: Vec<String> = in_order.iter().map(|ms| format!("{ms:.1}")).collect();
            println!("      GPU ms by frame: {}", each.join(" "));
        }
    }
    println!(
        "    last scene: {:?}; offscreen {}; effect pool peak {:.1} MB",
        stats.bytes,
        stats.offscreen,
        effect_target_peak() as f64 / 1e6
    );
}
