//! `… -- layers [1080|4k] <main video> <video>... [--frames <n>]`: what four video layers cost per frame. The main video
//! Fit in a portrait frame over its own blurred picture (the Blur background), and up to four videos over it as
//! picture-in-picture layers, each cropped and moving on motion keys (a drift and a breath of scale), all playing from
//! their starts. Frames in order at 30 fps through the real GPU backend, each waiting for its decoded pictures
//! (`wait_for_assets`) and its NV12 pixels back, as playback hands them over. 1080 draws 1080 × 1920, 4k twice that.
//!
//! Per case (the main video alone, then with 1, 2 and 4 layers): plan µs per frame, frame CPU ms (the plan handed in
//! until it is submitted), frame wall ms (until the pixels are back, decoding included) p50/p95/max, and the
//! frames a second that makes.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use klipolo_engine::plan::{PlanContext, frame_plan};
use klipolo_engine::present::Output;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::render::probe::probe_media;
use klipolo_engine::session::{PreviewBackend, RenderRequest};
use klipolo_model::crop::Crop;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::motion::{Key, Motion, MotionKeys};
use klipolo_model::project::{Asset, AssetKind, BackgroundKind, FormatKind, Project};
use klipolo_model::timeline::{
    Clip, ClipSource, Framing, FramingMode, Item, ItemContent, Track, TrackKind,
};

use super::measure::pct;
use super::project::secs;

const WARM: usize = 15;

fn asset(id: &str, path: &str) -> Asset {
    let m = probe_media(Path::new(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    Asset {
        id: id.into(),
        kind: AssetKind::Video,
        path: path.into(),
        name: id.into(),
        duration_us: secs(m.duration),
        width: m.width,
        height: m.height,
        fps: m.fps,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
        rotation: (m.rotation != 0).then_some(m.rotation),
    }
}

fn keys(values: &[(f64, f64)]) -> Vec<Key> {
    values
        .iter()
        .map(|&(t, v)| Key {
            t_us: secs(t),
            v,
            ease: None,
        })
        .collect()
}

/// The main video Fit in portrait over its blurred picture, and `layers` of `videos` over it.
fn project(main: &str, videos: &[String], layers: usize, seconds: f64) -> Project {
    let mut p = Project::new("Layers");
    p.settings.formats = vec![FormatKind::Portrait];
    p.settings.primary_format = FormatKind::Portrait;
    p.background.kind = BackgroundKind::Blur;
    p.assets = vec![asset("main", main)];
    let mut clip = Clip::new(ClipSource::Asset { id: "main".into() }, 0, secs(seconds));
    clip.id = "clip".into();
    clip.framing.insert(
        FormatKind::Portrait,
        Framing {
            mode: FramingMode::Fit,
            focus_x: 0.5,
            focus_y: 0.5,
            follow: None,
        },
    );
    p.timeline.main = vec![clip];
    let boxes = [(0.04, 0.03), (0.52, 0.03), (0.04, 0.74), (0.52, 0.74)];
    for k in 0..layers {
        let id = format!("v{k}");
        p.assets.push(asset(&id, &videos[k % videos.len()]));
        let (x, y) = boxes[k];
        let mut layout = Layout::new(
            Anchor::TopLeft,
            BoxN {
                x,
                y,
                w: 0.44,
                h: 0.22,
            },
        );
        layout.motion = Some(Motion {
            keys: Some(MotionKeys {
                x: keys(&[(0.0, 0.0), (seconds / 2.0, 0.02), (seconds, 0.0)]),
                scale: keys(&[(0.0, 1.0), (seconds / 2.0, 1.06), (seconds, 1.0)]),
                ..MotionKeys::default()
            }),
            ..Motion::default()
        });
        let mut track = Track::new(TrackKind::Overlay, format!("Video {k}"));
        track.items = vec![Item {
            id: format!("layer-{k}"),
            start_us: 0,
            duration_us: secs(seconds),
            attach: None,
            layout,
            content: ItemContent::Video {
                asset_id: id,
                source_in_us: 0,
                speed: 1.0,
                volume: 1.0,
                muted: true,
                mirror: false,
                fill: true,
                crop: Some(Crop {
                    left: 0.08,
                    top: 0.04,
                    right: 0.08,
                    bottom: 0.04,
                }),
                tie: None,
            },
            effects: Vec::new(),
        }];
        p.timeline.tracks.push(track);
    }
    klipolo_model::apply_ops(&p, &[]).expect("a checked project")
}

pub async fn run(args: &[String]) {
    let k = if args.iter().any(|a| a.eq_ignore_ascii_case("4k")) {
        2
    } else {
        1
    };
    let frames: usize = args
        .iter()
        .position(|a| a == "--frames")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(150);
    let files: Vec<String> = args
        .iter()
        .scan(false, |skip, a| {
            let keep = !*skip && !["1080", "4k", "4K", "--frames"].contains(&a.as_str());
            *skip = a == "--frames";
            Some(keep.then(|| a.clone()))
        })
        .flatten()
        .collect();
    let Some((main, videos)) = files.split_first().filter(|(_, v)| !v.is_empty()) else {
        eprintln!("usage: layers [1080|4k] <main video> <video>... [--frames <n>]");
        std::process::exit(2);
    };
    let size = (1080 * k, 1920 * k);
    let seconds = (WARM + frames) as f64 / 30.0 + 1.0;
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .expect("GPU")
        .wait_for_assets();
    println!(
        "layers at {} × {} on {} (Blur background, main Fit; {WARM} warm-up + {frames} frames in order at 30 fps)",
        size.0,
        size.1,
        backend.adapter_name()
    );
    for layers in [0, 1, 2, 4] {
        let ctx = PlanContext::new(Arc::new(project(main, videos, layers, seconds)));
        let (mut plan_us, mut cpu, mut wall) = (Vec::new(), Vec::new(), Vec::new());
        let run = Instant::now();
        let mut timed_from = None;
        for f in 0..WARM + frames {
            if f == WARM {
                timed_from = Some(Instant::now());
            }
            let at = secs(f as f64 / 30.0);
            let started = Instant::now();
            let plan = frame_plan(&ctx, at, FormatKind::Portrait);
            let planned = started.elapsed().as_secs_f64() * 1e6;
            let rendered = backend
                .render(RenderRequest {
                    plan,
                    width: size.0,
                    height: size.1,
                    output: Output::Nv12,
                })
                .await
                .expect("a frame");
            let submitted = started.elapsed().as_secs_f64() * 1e3;
            let mut pixels = rendered.pixels;
            pixels.wait().await.expect("its pixels");
            if f >= WARM {
                plan_us.push(planned);
                cpu.push(submitted);
                wall.push(started.elapsed().as_secs_f64() * 1e3);
            }
        }
        let fps = frames as f64 / timed_from.map_or(run, |t| t).elapsed().as_secs_f64();
        for v in [&mut plan_us, &mut cpu, &mut wall] {
            v.sort_by(f64::total_cmp);
        }
        println!(
            "  {layers} layer(s): plan {:.1} µs | frame CPU p50/p95/max {:.2}/{:.2}/{:.2} ms | wall {:.1}/{:.1}/{:.1} ms \
             | {fps:.1} frames a second",
            pct(&plan_us, 0.5),
            pct(&cpu, 0.5),
            pct(&cpu, 0.95),
            cpu.last().copied().unwrap_or(0.0),
            pct(&wall, 0.5),
            pct(&wall, 0.95),
            wall.last().copied().unwrap_or(0.0),
        );
    }
}
