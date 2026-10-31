//! `cargo run -p klipolo-engine --example perf_harness -- effects`: what effect passes and transitions cost
//! per frame at 720p. One still frame of a 1080p screen recording is drawn again and again through the real GPU
//! backend (decoders warm, pixels read back and waited for), with no effects and then with effect layers or a
//! transition, so the difference is the effect stage: the layer drawn alone, its shader passes and its quad. Also
//! reported: process heap allocations per frame and GPU objects the effect stage made once warm (none expected).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use klipolo_engine::plan::{FramePlan, PlanContext, frame_plan};
use klipolo_engine::present::Output;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::render::device::RenderDevice;
use klipolo_engine::render::effects::{EffectStage, gpu_objects_made};
use klipolo_engine::session::{PreviewBackend, RenderRequest};
use klipolo_model::contrib::builtin;
use klipolo_model::layout::{Anchor, BoxN, Layout};
use klipolo_model::ops::Op;
use klipolo_model::params::ParamValue;
use klipolo_model::project::{Asset, AssetKind, FormatKind, Project};
use klipolo_model::time::Us;
use klipolo_model::timeline::{
    Clip, ClipSource, EffectInstance, Item, ItemContent, Track, TrackKind, Transition,
};

use super::measure::{allocs, pct};
use super::project::{fixture, secs};

const WARM: usize = 30;
const FRAMES: usize = 150;

pub(super) fn effect(id: &str, contrib: &str) -> EffectInstance {
    EffectInstance {
        id: id.into(),
        ..EffectInstance::new(contrib)
    }
}

/// A 4 s 1080p screen recording on the main track.
pub(super) fn base() -> Project {
    let mut p = Project::new("Effects");
    p.assets = vec![Asset {
        id: "screen".into(),
        kind: AssetKind::Video,
        path: fixture("bars-30fps-0.5gop.mp4"),
        name: "screen".into(),
        duration_us: secs(4.0),
        width: 1920,
        height: 1080,
        fps: 30.0,
        has_audio: false,
        audio_lead_us: None,
        made_with: None,
        rotation: None,
    }];
    let mut clip = Clip::new(
        ClipSource::Asset {
            id: "screen".into(),
        },
        0,
        secs(4.0),
    );
    clip.id = "clip".into();
    p.timeline.main = vec![clip];
    p
}

/// `p` with an adjustment layer over `rect` carrying `effects`.
pub(super) fn with_adjustment(mut p: Project, rect: BoxN, effects: Vec<EffectInstance>) -> Project {
    let mut track = Track::new(TrackKind::Overlay, "Overlay 1");
    track.items = vec![Item {
        id: "adjust".into(),
        start_us: 0,
        duration_us: secs(4.0),
        attach: None,
        layout: Layout::new(Anchor::TopLeft, rect),
        content: ItemContent::Adjustment,
        effects,
    }];
    p.timeline.tracks = vec![track];
    p
}

/// The whole frame, as a layer box.
pub(super) fn full_frame() -> BoxN {
    BoxN {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    }
}

/// The base recording cut in two (0–1.5 s, then 2–4 s) and joined by a 1 s dissolve; drawn at the cut (1.5 s), both
/// sides decode the one file at two times (two lanes), as a split screen recording does.
fn dissolve_at_a_cut() -> Project {
    let mut p = base();
    let mut next = p.timeline.main[0].clone();
    next.id = "next".into();
    next.source_in_us = secs(2.0);
    p.timeline.main[0].source_out_us = secs(1.5);
    p.timeline.main.push(next);
    let dissolve = Op::SetTransition {
        clip_id: "clip".into(),
        transition: Some(Transition {
            effect: builtin::TRANSITION_DISSOLVE.into(),
            duration_us: secs(1.0),
            params: BTreeMap::new(),
        }),
    };
    klipolo_model::apply_ops(&p, &[dissolve]).expect("the dissolve fits")
}

/// Each variant and the instant it is drawn at.
fn variants() -> Vec<(&'static str, Project, Us)> {
    let full = full_frame();
    let lower = BoxN {
        x: 0.0,
        y: 0.5,
        w: 1.0,
        h: 0.5,
    };
    let blurred = |mut p: Project| {
        p.timeline.main[0].effects = vec![effect("blur", builtin::EFFECT_BLUR)];
        p
    };
    let pixelate = || vec![effect("px", builtin::EFFECT_PIXELATE)];
    let mut gradient = with_adjustment(base(), full, Vec::new());
    gradient.timeline.tracks[0].items[0].content = ItemContent::generator(
        builtin::GENERATOR_GRADIENT.into(),
        BTreeMap::from([("angle".into(), ParamValue::Number(45.0))]),
    );
    let at = secs(1.0);
    vec![
        ("no effects", base(), at),
        ("clip blur (radius 12)", blurred(base()), at),
        (
            "clip vignette",
            {
                let mut p = base();
                p.timeline.main[0].effects = vec![effect("v", builtin::EFFECT_VIGNETTE)];
                p
            },
            at,
        ),
        (
            "adjustment pixelate, full frame",
            with_adjustment(base(), full, pixelate()),
            at,
        ),
        ("gradient generator, full frame", gradient, at),
        (
            "two layers: clip blur + pixelate on the lower half",
            with_adjustment(blurred(base()), lower, pixelate()),
            at,
        ),
        (
            "two layers: clip vignette + pixelate, full frame",
            with_adjustment(
                {
                    let mut p = base();
                    p.timeline.main[0].effects = vec![effect("v", builtin::EFFECT_VIGNETTE)];
                    p
                },
                full,
                pixelate(),
            ),
            at,
        ),
        (
            "two layers, worst case: clip blur + full-frame blur",
            with_adjustment(
                blurred(base()),
                full,
                vec![effect("blur", builtin::EFFECT_BLUR)],
            ),
            at,
        ),
        (
            "transition: dissolve at a cut (one file, two times)",
            dissolve_at_a_cut(),
            secs(1.5),
        ),
    ]
}

/// One frame of `plan` at 720p: wall ms (submit → pixels), heap allocations and effect-stage GPU objects made.
async fn frame(backend: &mut GpuBackend, plan: &FramePlan) -> (f64, u64, u64) {
    let req = RenderRequest {
        plan: plan.clone(),
        width: 1280,
        height: 720,
        output: Output::Rgba,
    };
    let (a0, _) = allocs();
    let made0 = gpu_objects_made();
    let started = Instant::now();
    let pixels = backend
        .render(req)
        .await
        .expect("render")
        .finish()
        .await
        .expect("pixels");
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let (a1, _) = allocs();
    drop(pixels);
    (ms, a1 - a0, gpu_objects_made() - made0)
}

pub async fn run() {
    // What compiling every built-in effect pipeline costs (`EffectStage::warm`: the GPU warm-up's, on its own device).
    let dev = RenderDevice::new().await.expect("GPU");
    let started = Instant::now();
    EffectStage::new(&dev, klipolo_engine::render::mode::RenderMode::Contained).warm();
    println!(
        "built-in effect pipelines compiled at engine start in {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    drop(dev);
    let mut backend = GpuBackend::new(klipolo_engine::render::mode::RenderMode::Contained)
        .await
        .expect("GPU")
        .wait_for_assets();
    println!(
        "effects at 1280×720 on {} ({WARM} warm-up + {FRAMES} frames each, interleaved with the plain frame; submit → \
         pixels waited)",
        backend.adapter_name()
    );
    let plain = frame_plan(
        &PlanContext::new(Arc::new(base())),
        secs(1.0),
        FormatKind::Landscape,
    );
    for (name, project, at_us) in variants() {
        let plan = frame_plan(
            &PlanContext::new(Arc::new(project)),
            at_us,
            FormatKind::Landscape,
        );
        let (mut base_ms, mut ms, mut delta) = (Vec::new(), Vec::new(), Vec::new());
        let (mut base_heap, mut heap, mut made) = (0, 0, 0);
        for k in 0..WARM + FRAMES {
            let (b, b_heap, _) = frame(&mut backend, &plain).await;
            let (t, t_heap, t_made) = frame(&mut backend, &plan).await;
            if k >= WARM {
                base_ms.push(b);
                ms.push(t);
                delta.push(t - b);
                base_heap += b_heap;
                heap += t_heap;
                made += t_made;
            }
        }
        for v in [&mut base_ms, &mut ms, &mut delta] {
            v.sort_by(f64::total_cmp);
        }
        let per_frame = |n: u64| n as f64 / FRAMES as f64;
        println!(
            "  {name:<52} p50 {:5.2} ms (plain {:5.2})  p95 {:5.2} ms  added p50 {:+5.2} ms  {:5.1} allocs/frame ({:+5.1})  \
             {made} GPU objects made",
            pct(&ms, 0.5),
            pct(&base_ms, 0.5),
            pct(&ms, 0.95),
            pct(&delta, 0.5),
            per_frame(heap),
            per_frame(heap) - per_frame(base_heap),
        );
    }
}
