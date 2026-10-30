//! The measurement's time: a point stops once its p95 is surely Over, nothing is measured past the
//! request's budget, an export size stops at a frame over the step limit, and the worker's own meter warms
//! each size once per request and keeps that cold frame.

use std::time::{Duration, Instant};

use klipolo_engine::make::budget::CostClass;
use klipolo_engine::make::measure::FakeMeter;
use klipolo_engine::make::protocol::MeasureSpec;

use crate::support::*;

/// 3.3 ns a pixel is 3 ms at preview size, over Normal's 2 ms with no half size: two frames over the limit prove
/// the p95 of 30 is over it, so the defaults stop there (the step's frame, then two) and nothing else is measured.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_point_stops_as_soon_as_its_p95_is_surely_over() {
    let mut w = InProcessWorker::new(FakeMeter::new(3.3)).await;
    let c = w.compile(draft_def(), WAVES_BODY).await;
    let cost = c.compiled().unwrap().cost.clone().unwrap();
    assert_eq!(cost.class, CostClass::Over, "{cost:?}");
    assert_eq!(w.meter_frames_at((1280, 720)), 1 + 2, "not 1 + 30");
}

/// A legal draft whose full measurement takes about 200 frames of 20 ms here (five points of 34, then 30 at
/// 1080p) is answered once its 2 s budget is spent (at most a point's steps later), never failed, well inside the
/// compile deadline, and says what it left unmeasured (that counts as Over).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_slow_measurement_ends_at_its_budget_with_a_note() {
    let delay = Duration::from_millis(20);
    let mut w = InProcessWorker::new(FakeMeter::new(1.0).delay(delay)).await;
    let spec = MeasureSpec {
        budget_ms: 2_000,
        ..MeasureSpec::default()
    };
    let started = Instant::now();
    let c = w
        .compile_with(branch_gated_def(), BRANCH_GATED_BODY, spec)
        .await;
    let took = started.elapsed();
    let cost = c.compiled().unwrap().cost.clone().unwrap();
    assert_eq!(w.failed_replies(), 0);
    assert!(
        w.frames_timed() <= 2_000 / 20 + 6,
        "{} frames timed",
        w.frames_timed()
    );
    assert!(took < Duration::from_secs(8), "{took:?}");
    assert!(
        cost.advice.iter().any(|a| a.contains("weren't measured")),
        "{:?}",
        cost.advice
    );
    // The export size the time cut is said as an estimate, not counted as a setting combination.
    assert!(
        cost.advice
            .iter()
            .any(|a| a.contains("1920×1080 time is estimated from the preview")),
        "{:?}",
        cost.advice
    );
}

/// An export size stops at its first frame over the step limit (50 ms) rather than drawing all 30.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn an_export_size_stops_at_a_frame_over_the_step_limit() {
    let meter = FakeMeter::new(0.4).at_size((1920, 1080), 60.0);
    let mut w = InProcessWorker::new(meter).await;
    let c = w.compile(draft_def(), WAVES_BODY).await;
    assert!(c.compiled().unwrap().ok);
    assert_eq!(w.meter_frames_at((1920, 1080)), 1);
}

/// The worker's own meter (`GpuMeter`) on lavapipe: a frame's GPU time from its first and last passes'
/// timestamps where they are trusted and plausible, else the wall clock; finite and never negative. Each size is warmed
/// once per request, and that cold frame is kept for the advice.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_gpu_meter_times_frames_and_warms_each_size_once_a_request() {
    use klipolo_engine::make::measure::{FrameReq, GpuMeter, Meter};
    let mut h = headless_alone().await;
    let layer = draft(draft_def(), WAVES_BODY);
    layer.prepare(&mut h, true, false);
    let project = std::sync::Arc::new(layer.project());
    let mut meter = GpuMeter::new();
    meter.begin();
    for size in [(16, 16), (64, 64), (16, 16)] {
        let req = FrameReq::new(project.clone(), &layer, layer.middle(), size);
        meter.warm(&mut h, &req).await.unwrap();
        let sample = meter.frame(&mut h, &req).await.unwrap();
        println!(
            "{size:?}: {sample:?} (timestamps: {})",
            h.device().has_timestamps()
        );
        assert!(
            sample.gpu_ms.is_finite() && sample.gpu_ms >= 0.0,
            "{sample:?}"
        );
    }
    let sizes: Vec<(u32, u32)> = meter.cold().iter().map(|c| c.size).collect();
    assert_eq!(sizes, vec![(16, 16), (64, 64)], "16² warmed once");
    meter.begin();
    assert!(meter.cold().is_empty(), "a new request starts cold");
}
