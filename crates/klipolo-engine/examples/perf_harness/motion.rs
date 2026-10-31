//! `cargo run -p klipolo-engine --example perf_harness --release -- motion`: what layer motion and blend modes cost per
//! frame. Four image layers over a 1080p screen recording, still and then moving (a slide in from the
//! left, keys that move and grow it, a pulse loop, and a screen-blended layer that wiggles), and the moving scene
//! with its fourth layer overlay-blended instead. For each: planning time and heap allocations per frame (the plan
//! alone, 2,000 frames across the 4 s), then 720p frames through the real GPU backend (decoders warm, pixels read
//! back and waited for) with their render passes (`GpuBackend::passes_last_frame`).
//!
//! Budget: moving layers add at most 0.1 ms of plan time per frame and no GPU pass; overlay adds its
//! read-below pass and the layer pass after it.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use klipolo_engine::plan::{FramePlan, PlanContext, frame_plan};
use klipolo_engine::present::Output;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::session::{PreviewBackend, RenderRequest};
use klipolo_model::contrib::builtin;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::motion::{Blend, Key, Motion, MotionKeys, MotionUse};
use klipolo_model::params::ParamValue;
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::time::Us;
use klipolo_model::timeline::{Item, ItemContent, Track, TrackKind};

use super::effects::base;
use super::measure::{allocs, pct};
use super::project::secs;

const PLANS: usize = 2_000;
const WARM: usize = 30;
const FRAMES: usize = 150;

fn key(t: f64, v: f64) -> Key {
    Key {
        t_us: secs(t),
        v,
        ease: None,
    }
}

fn preset(word: &str, seconds: f64, params: &[(&str, ParamValue)]) -> MotionUse {
    MotionUse {
        preset: builtin::motion(word),
        params: params
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
        duration_us: secs(seconds),
        ease: None,
    }
}

/// The base recording with four 4 s image layers on their own tracks, each given `(motion, blend)`.
fn scene(png: &Path, layers: [(Option<Motion>, Option<Blend>); 4]) -> Project {
    let mut p = base();
    p.assets.push(Asset {
        id: "logo".into(),
        kind: AssetKind::Image,
        path: png.display().to_string(),
        name: "logo".into(),
        duration_us: 0,
        width: 512,
        height: 512,
        fps: 0.0,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    });
    let boxes = [(0.05, 0.1), (0.55, 0.1), (0.05, 0.55), (0.55, 0.55)];
    p.timeline.tracks = layers
        .into_iter()
        .zip(boxes)
        .enumerate()
        .map(|(k, ((motion, blend), (x, y)))| {
            let mut layout = Layout::new(
                Anchor::TopLeft,
                BoxN {
                    x,
                    y,
                    w: 0.3,
                    h: 0.3,
                },
            );
            layout.motion = motion;
            layout.blend = blend;
            let mut track = Track::new(TrackKind::Overlay, format!("Layer {k}"));
            track.items = vec![Item {
                id: format!("layer-{k}"),
                start_us: 0,
                duration_us: secs(4.0),
                attach: None,
                layout,
                content: ItemContent::Image {
                    asset_id: "logo".into(),
                },
                effects: Vec::new(),
            }];
            track
        })
        .collect();
    p
}

fn moving(fourth: Blend) -> [(Option<Motion>, Option<Blend>); 4] {
    let slide = Motion {
        enter: Some(preset(
            "slide",
            1.0,
            &[("from", ParamValue::Text("left".into()))],
        )),
        ..Motion::default()
    };
    let keys = Motion {
        keys: Some(MotionKeys {
            x: vec![key(0.0, 0.0), key(4.0, 0.1)],
            scale: vec![key(0.0, 1.0), key(4.0, 1.5)],
            ..MotionKeys::default()
        }),
        ..Motion::default()
    };
    let pulse = Motion {
        looping: Some(preset("pulse", 2.0, &[])),
        ..Motion::default()
    };
    let wiggle = Motion {
        looping: Some(preset("wiggle", 1.0, &[])),
        ..Motion::default()
    };
    [
        (Some(slide), None),
        (Some(keys), None),
        (Some(pulse), None),
        (Some(wiggle), Some(fourth)),
    ]
}

/// Planning `PLANS` frames across the 4 s: µs and heap allocations per frame.
fn plan_cost(ctx: &PlanContext) -> (f64, f64) {
    let at = |k: usize| secs(0.05 + 3.9 * k as f64 / PLANS as f64);
    let _ = frame_plan(ctx, at(0), FormatKind::Landscape);
    let (a0, _) = allocs();
    let started = Instant::now();
    for k in 0..PLANS {
        std::hint::black_box(frame_plan(ctx, at(k), FormatKind::Landscape));
    }
    let us = started.elapsed().as_secs_f64() * 1e6 / PLANS as f64;
    let (a1, _) = allocs();
    (us, (a1 - a0) as f64 / PLANS as f64)
}

/// One frame of `plan` at 720p: wall ms (submit → pixels) and its render passes.
async fn frame(backend: &mut GpuBackend, plan: &FramePlan) -> (f64, u32) {
    let req = RenderRequest {
        plan: plan.clone(),
        width: 1280,
        height: 720,
        output: Output::Rgba,
    };
    let started = Instant::now();
    let pixels = backend
        .render(req)
        .await
        .expect("render")
        .finish()
        .await
        .expect("pixels");
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    drop(pixels);
    (ms, backend.passes_last_frame())
}

pub async fn run() {
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("logo.png");
    image::RgbaImage::from_fn(512, 512, |x, y| {
        image::Rgba([(x / 2) as u8, (y / 2) as u8, 160, 220])
    })
    .save(&png)
    .unwrap();
    let still = || [(None, None), (None, None), (None, None), (None, None)];
    let variants = [
        ("still (4 layers)", scene(&png, still())),
        (
            "moving: slide, keys, pulse, screen",
            scene(&png, moving(Blend::Screen)),
        ),
        (
            "moving, the fourth overlay-blended",
            scene(&png, moving(Blend::Overlay)),
        ),
    ];
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .expect("GPU")
        .wait_for_assets();
    println!(
        "motion at 1280×720 on {} ({PLANS} plans; {WARM} warm-up + {FRAMES} frames each, submit → pixels waited)",
        backend.adapter_name()
    );
    let mut still_plan_us = None;
    for (name, project) in variants {
        let ctx = PlanContext::new(Arc::new(project));
        let (us, plan_allocs) = plan_cost(&ctx);
        let base_us = *still_plan_us.get_or_insert(us);
        let mut ms = Vec::new();
        let mut passes = Vec::new();
        for k in 0..WARM + FRAMES {
            let at: Us = secs(0.05 + 3.9 * (k % 117) as f64 / 117.0);
            let plan = frame_plan(&ctx, at, FormatKind::Landscape);
            let (t, n) = frame(&mut backend, &plan).await;
            if k >= WARM {
                ms.push(t);
                passes.push(n);
            }
        }
        ms.sort_by(f64::total_cmp);
        passes.sort_unstable();
        println!(
            "  {name:<36} plan {us:6.2} µs/frame ({:+5.2} µs vs still), {plan_allocs:5.1} allocs/frame | render p50 {:6.2} ms \
             p95 {:6.2} ms | passes {}–{}",
            us - base_us,
            pct(&ms, 0.5),
            pct(&ms, 0.95),
            passes.first().copied().unwrap_or(0),
            passes.last().copied().unwrap_or(0),
        );
    }
}
