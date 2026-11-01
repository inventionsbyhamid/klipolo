//! Fenced timing. Where the GPU's timestamps aren't trusted (every Mac today), a frame is
//! timed from its submission to its completion, and on Metal wgpu's fence wait sleeps 1 ms between polls: a single
//! frame's time is quantised in steps of about 1.1–1.5 ms, so work that ends within one step adds nothing to it. The
//! measurement times batches instead: at each sample time a single frame and a batch of copies in one submission with
//! one wait, the cost a frame being the slope between them, every fixed cost (the wait included) cancelled.
//! `FakeMeter::quantised(1.0, 0.1)` times as such a GPU: 1 ms steps, 0.1 ms of fixed cost a submission.

use klipolo_engine::make::budget::{Cost, CostClass};
use klipolo_engine::make::measure::FakeMeter;

use crate::support::*;

const FLAT: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, 0.5, 1.0); }";
/// The poll's step and a submission's fixed cost, in ms.
const STEP_MS: f64 = 1.0;
const FIXED_MS: f64 = 0.1;
/// A slope's error over a span of 16 frames: under one step over the span.
const RESOLUTION_MS: f64 = STEP_MS / 16.0;

/// A meter under which the flat generator costs `ms` a frame at preview size (1280 × 720).
fn costing(ms: f64) -> FakeMeter {
    FakeMeter::new(ms * 1e6 / (1280.0 * 720.0))
}

async fn cost_under(meter: FakeMeter) -> Cost {
    let mut w = InProcessWorker::new(meter).await;
    let c = w.compile(draft_def_with("[]"), FLAT).await;
    c.compiled()
        .unwrap_or_else(|| panic!("{:?}", c.failure()))
        .cost
        .clone()
        .unwrap()
}

/// The cases under 1 ms steps: 0.1 ms is Light, 0.8 ms (one step's worth: a single frame reads the same as an
/// empty one) and 1.3 ms are Normal, 3 ms is over Normal (Over: this generator may not run at half size). Each figure
/// within Normal is the body's own to the batches' resolution.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_quantised_fence_still_tells_light_from_normal_and_over() {
    let mut wrong = Vec::new();
    for (ms, class) in [
        (0.1, CostClass::Light),
        (0.8, CostClass::Normal),
        (1.3, CostClass::Normal),
        (3.0, CostClass::Over),
    ] {
        let cost = cost_under(costing(ms).quantised(STEP_MS, FIXED_MS)).await;
        let off = (cost.decided_ms - ms).abs() > RESOLUTION_MS + 1e-9;
        let batches = cost.advice.iter().any(|a| a.contains("batches"));
        if cost.class != class || (ms < 2.0 && off) || !batches {
            wrong.push(format!(
                "{ms} ms: {:?} at {:.4} ms (expected {class:?}), batches named: {batches}",
                cost.class, cost.decided_ms
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// When the batches are noisier than their resolution allows (here every 3rd batch is held 5 ms by other work, the
/// baseline's included), a body that reads Light isn't trusted to be: it counts as Normal, never Light, and says so.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_cost_the_batches_cant_resolve_is_never_light() {
    let meter = costing(0.1)
        .quantised(STEP_MS, FIXED_MS)
        .batch_stall(3, 5.0);
    let cost = cost_under(meter).await;
    assert_eq!(cost.class, CostClass::Normal, "{cost:?}");
    assert!(
        cost.decided_ms <= 0.5,
        "its figure is still what was measured: {cost:?}"
    );
    assert!(
        cost.advice.iter().any(|a| a.contains("never Light")),
        "{:?}",
        cost.advice
    );
}

/// Batches are bounded: at most 17 frames (a span of 16 over the single frame), never predicted to keep the GPU busy
/// past the 50 ms step limit (a 3 ms body's 1080p batches are shorter), and at most 8 sample times at each size the
/// measurement draws (preview and 1080p here, each with its baseline). A trusted meter never batches.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn batches_are_bounded_and_only_where_the_timing_is_fenced() {
    for ms in [0.1, 3.0] {
        let meter = costing(ms).quantised(STEP_MS, FIXED_MS);
        let batches = meter.batches();
        let cost = cost_under(meter).await;
        let batches = batches.lock().unwrap().clone();
        assert!(!batches.is_empty(), "{ms} ms: {cost:?}");
        assert!(batches.len() <= 4 * 8, "{ms} ms: {} batches", batches.len());
        for (frames, busy) in &batches {
            assert!(*frames <= 17, "{ms} ms: a batch of {frames}");
            assert!(*busy <= 50.0, "{ms} ms: a batch busy for {busy} ms");
        }
    }
    let meter = costing(0.1);
    let batches = meter.batches();
    let cost = cost_under(meter).await;
    assert_eq!(cost.class, CostClass::Light);
    assert!(
        batches.lock().unwrap().is_empty(),
        "trusted timing never batches"
    );
}

/// The real device draws a batch: several copies in one submission with one wait, as many passes a frame as one copy,
/// and no warning.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_batch_draws_its_copies_in_one_submission() {
    let mut h = headless_alone().await;
    let layer = draft(draft_def_with("[]"), FLAT);
    layer.prepare(&mut h, true, false);
    let project = std::sync::Arc::new(layer.project());
    let (at, format, size) = (layer.middle(), layer.format, (256, 144));
    h.render_timed(project.clone(), at, format, size)
        .await
        .unwrap();
    let passes = h.passes_last_frame();
    let batch = h
        .render_batch(project.clone(), at, format, size, 8)
        .await
        .unwrap();
    assert!(batch.warnings.is_empty(), "{:?}", batch.warnings);
    assert!(!batch.timestamps && batch.gpu_ms > 0.0, "{batch:?}");
    assert_eq!(
        h.passes_last_frame(),
        passes,
        "each copy is the whole frame"
    );
}

/// The cost keeps what the batches measured (the raw times, so a later trusted run can compare): 8 sample times at
/// preview size, the first a single frame and a batch of 17, the rest batches of 7 once the first showed the body over
/// Light, the baseline's too (all at the full span, steady); 1080p's likewise.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_fenced_cost_keeps_its_batches() {
    let cost = cost_under(costing(0.8).quantised(STEP_MS, FIXED_MS)).await;
    let f = cost.fenced.clone().unwrap_or_else(|| panic!("{cost:?}"));
    assert!(f.resolved && !f.held, "{f:?}");
    let preview = f.preview.as_ref().unwrap();
    assert_eq!(preview.samples.len(), 8, "{preview:?}");
    assert_eq!(preview.samples[0].frames, 17, "{preview:?}");
    assert!(
        preview.samples[1..].iter().all(|b| b.frames == 7),
        "{preview:?}"
    );
    assert_eq!(preview.baseline.len(), 8, "{preview:?}");
    assert!(
        preview.baseline.iter().all(|b| b.frames == 17),
        "{preview:?}"
    );
    assert!(preview.resolved && preview.baseline_steady, "{preview:?}");
    assert!(
        f.p1080.as_ref().is_some_and(|b| b.samples.len() == 8),
        "{f:?}"
    );
    assert!(f.half.is_none() && f.p2160.is_none(), "{f:?}");
}

/// The worker's own meter times the batched path on a real device (`GpuMeter::always_fenced`: lavapipe's timestamps
/// are trusted): batches of at most 17 frames, kept with the measurement, and no failure.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_gpu_meter_times_batches_on_a_real_device() {
    use klipolo_engine::make::measure::{GpuMeter, measure};
    use klipolo_engine::make::protocol::MeasureSpec;
    let mut h = headless_alone().await;
    let layer = draft(draft_def_with("[]"), FLAT);
    let mut meter = GpuMeter::always_fenced();
    let m = measure(&mut meter, &mut h, &layer, &MeasureSpec::default(), &[])
        .await
        .unwrap();
    let f = m.fenced.clone().unwrap_or_else(|| panic!("{m:?}"));
    if let Some(preview) = &f.preview {
        assert!(!preview.samples.is_empty(), "{f:?}");
        assert!(preview.samples.iter().all(|b| b.frames <= 17), "{f:?}");
        assert!(!preview.baseline.is_empty(), "{f:?}");
    }
    println!("lavapipe, fenced: {:?} {:?}; {f:?}", m.class, m.decided);
}

/// Iris-like singles (1.5 ms poll steps after 1.6 ms of fixed cost: 3.0 ms singles, so 16 copies fit
/// the 50 ms limit, a span of 15) still resolve Light's precision (3.0 / 15 = 0.2 ms): a 0.1 ms body is Light, not held
/// at Normal, and a 0.8 ms body Normal.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn iris_like_singles_still_resolve_light() {
    let mut wrong = Vec::new();
    for (ms, class) in [(0.1, CostClass::Light), (0.8, CostClass::Normal)] {
        let cost = cost_under(costing(ms).quantised(1.5, 1.6)).await;
        let off = (cost.decided_ms - ms).abs() > 0.2 + 1e-9;
        if cost.class != class || off {
            wrong.push(format!(
                "{ms} ms: {:?} at {:.4} ms (expected {class:?}); {:?}",
                cost.class, cost.decided_ms, cost.advice
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A batch whose copies all encode fewer passes than the single frame it is differenced against (as if
/// every copy reused a picture the single drew) gives no slope: the single frame alone is the figure, an upper bound, and
/// a 0.1 ms body isn't read Light from it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_batch_whose_copies_skip_a_pass_of_the_single_gives_no_slope() {
    let cost = cost_under(
        costing(0.1)
            .quantised(STEP_MS, FIXED_MS)
            .copies_skip_a_pass(),
    )
    .await;
    assert_ne!(cost.class, CostClass::Light, "{cost:?}");
    let preview = cost.fenced.as_ref().and_then(|f| f.preview.clone());
    assert!(
        preview.is_some_and(|b| b.samples.iter().all(|s| s.frames == 0)),
        "{cost:?}"
    );
}

/// A body whose first sample is already over Light by more than its resolution can't be Light, so the
/// rest of its samples are timed with short batches (7 frames), which keeps a draft at the setting caps within the
/// compile budget; its figure stays the body's to within the short batch's precision.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn a_body_clearly_over_light_is_timed_with_short_batches() {
    let meter = costing(1.5).quantised(1.5, 1.6);
    let batches = meter.batches();
    let cost = cost_under(meter).await;
    let layer: Vec<u32> = batches
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, busy)| *busy > 0.0)
        .map(|(frames, _)| *frames)
        .collect();
    assert_eq!(cost.class, CostClass::Normal, "{cost:?}");
    assert!(
        (cost.decided_ms - 1.5).abs() <= 0.3,
        "1.5 ms read {} ms",
        cost.decided_ms
    );
    assert!(
        layer.iter().filter(|&&f| f == 7).count() >= 7,
        "the layer's batches: {layer:?}"
    );
}

/// A stepped (`fps`) generator keeps its picture for a step, but a timed frame never reuses it: every
/// copy of a batch draws the pass, as a frame that draws it does. The measured copy drops `fps` (it never keeps a
/// picture); the shader as it is, which does keep one on plain frames, is held to the same while timed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn every_copy_of_a_batch_draws_a_stepped_generators_pass() {
    let def = klipolo_model::contrib::ContribDef {
        fps: Some(6),
        ..draft_def_with("[]")
    };
    let body =
        "fn effect(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(uv, fract(kl.time), 1.0); }";
    let layer = draft(def, body);
    let mut h = headless_alone().await;
    let (at, format, size) = (layer.middle(), layer.format, (128, 72));
    // A frame that draws the pass: the measured copy's.
    layer.prepare(&mut h, true, false);
    let measured = std::sync::Arc::new(layer.project());
    h.render_timed(measured.clone(), at, format, size)
        .await
        .unwrap();
    let drawn = h.passes_last_frame();
    let batch = h.render_batch(measured, at, format, size, 6).await.unwrap();
    assert_eq!(
        batch.passes,
        vec![drawn; 6],
        "every copy of the measured copy's batch"
    );
    // The shader as it is keeps its picture for the step on plain frames (so this test can see a reuse)...
    layer.prepare(&mut h, false, false);
    let as_is = std::sync::Arc::new(layer.project());
    h.render(as_is.clone(), at, format, size).await.unwrap();
    h.render(as_is.clone(), at, format, size).await.unwrap();
    assert!(
        h.passes_last_frame() < drawn,
        "a plain frame within the step reuses the kept picture"
    );
    // ...but no timed frame does.
    h.render_timed(as_is.clone(), at, format, size)
        .await
        .unwrap();
    assert_eq!(
        h.passes_last_frame(),
        drawn,
        "a timed single frame draws the pass"
    );
    let batch = h.render_batch(as_is, at, format, size, 6).await.unwrap();
    assert_eq!(
        batch.passes,
        vec![drawn; 6],
        "every copy of the batch draws the pass, none reuses the picture"
    );
}

/// The capped draft the Mac checks time really reaches the caps under fenced timing (Iris-like
/// 1.5 ms steps; a ~1.5 ms body): its defaults are within class, so 12 combinations and its probes are measured (each
/// stepped from 16 × 16), the corner cap is noted, nothing ran out, and the envelope holds the ranges of `c` and `d`
/// and the choices of `mode`, `x` and `y`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a GPU (lavapipe in CI)"]
async fn the_capped_draft_reaches_the_caps_under_fenced_timing() {
    use klipolo_engine::make::budget::{Budget, CostClass};
    use klipolo_engine::make::envelope::costly_list;
    use klipolo_engine::make::measure::{COMPILE_BUDGET, Deadline, Decided, measure_noted};
    use klipolo_engine::make::protocol::MeasureSpec;
    let mut h = headless_alone().await;
    let layer = draft(draft_def_with(CAP_SETTINGS), &cap_body(8, true));
    let facts = layer.facts();
    let costly = costly_list(
        &layer.def,
        facts.map_or(&[][..], |f| f.costly_params.as_slice()),
    );
    let mut meter = costing(1.5).quantised(1.5, 1.6);
    let log = meter.log();
    let deadline = Deadline::new(std::time::Instant::now(), 0, COMPILE_BUDGET);
    let (m, notes) = measure_noted(
        &mut meter,
        &mut h,
        &layer,
        &MeasureSpec::default(),
        &costly,
        deadline,
        &Budget::DEFAULT,
    )
    .await
    .unwrap();
    let points = log
        .lock()
        .unwrap()
        .iter()
        .filter(|f| f.size == (16, 16))
        .count();
    assert!(
        m.decided != Decided::Unmeasured && m.class != CostClass::Over,
        "{m:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("at most 12 corners")),
        "{notes:?}"
    );
    assert!(!notes.iter().any(|n| n.contains("ran out")), "{notes:?}");
    assert!(points > 12, "{points} combinations measured");
    for key in ["c", "d"] {
        assert!(
            m.envelope.ranges.contains_key(key),
            "{key}: {:?}",
            m.envelope
        );
    }
    for key in ["mode", "x", "y"] {
        assert!(
            m.envelope.allowed.contains_key(key),
            "{key}: {:?}",
            m.envelope
        );
    }
}
