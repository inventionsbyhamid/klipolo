//! The render worker's measurement on the Mac's own GPU (run by hand on a Mac after a Mac build): the worker's device
//! on the real adapter, whether it has timestamp queries, the waves preset's cost, and a heavy but legal body (a loop
//! of 4,096 trips of ~60 operations, just under the static cap) that must stop at or before the preview step, never
//! drawn as one long frame. And the budget table's calibration, with the shader presets' claimed classes.
//!
//! Every test here is `#[ignore]`d and run only on the Mac (`-- --ignored`); the file compiles on every platform, so CI
//! type-checks and lints it.

mod support;
use klipolo_engine::make::measure::{GpuMeter, Meter, measure};
use klipolo_engine::make::protocol::MeasureSpec;
use klipolo_engine::make::steps::{StepOutcome, stepped};
use support::*;

const HEAVY: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> {
    var p = uv;
    var c = 0.0;
    for (var i = 0; i < 4096; i++) {
        p = vec2<f32>(sin(p.x * 3.1 + c), cos(p.y * 2.7 - c));
        c = c + fract(p.x * p.y * 43758.5) * 0.001;
    }
    return vec4<f32>(vec3<f32>(fract(c)), 1.0);
}";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "Mac only: Metal timestamps"]
async fn the_worker_measures_on_the_real_gpu_and_stops_a_heavy_body_early() {
    let mut h = headless_alone().await;
    println!("adapter: {}", h.device().adapter_name());
    println!(
        "timestamp queries: {}; trusted for timing: {} (Metal's are trusted only once an Apple-silicon \
         run verifies them)",
        if h.device().has_timestamps() {
            "on"
        } else {
            "off"
        },
        h.device().timestamps_trusted()
    );
    let waves = draft(draft_def(), WAVES_BODY);
    let m = measure(
        &mut GpuMeter::new(),
        &mut h,
        &waves,
        &MeasureSpec::default(),
        &[],
    )
    .await
    .unwrap();
    println!(
        "waves: preview p50 {:.3} ms, p95 {:.3} ms; 1080p p95 {:.3} ms; peak {} bytes",
        m.preview.p50, m.preview.p95, m.p1080.p95, m.bytes.peak
    );
    let heavy = draft(draft_def_with("[]"), HEAVY);
    let out = stepped(
        &mut GpuMeter::new(),
        &mut h,
        &heavy,
        &[(16, 16), (64, 64), (256, 256), (1280, 720)],
        50.0,
    )
    .await;
    println!("heavy: {out:?}");
    assert!(
        matches!(out, StepOutcome::TooSlow { .. } | StepOutcome::Over { .. }),
        "the heavy body stops at or before the preview step: {out:?}"
    );
}

/// The budget table's calibration (run by hand on a Mac after a Mac build; it prints the table): the built-in blur at
/// radius 12, vignette and gradient, and the sample waves and sparkle bodies, measured as the worker measures a draft
/// on this GPU (`measure` with `GpuMeter`, its settings at their costly ends and probes) and classed by `classify`. The
/// table in `Budget` changes only after a hand run. Where the timing is fenced (Iris Plus), each row also shows the
/// deciding point's batches at preview size: their frames (fewest–most), the median single frame and batch, whether
/// they told Light, and whether the baseline was steady. Expected on Iris Plus: the vignette Light, or
/// Normal with its p95 within ~0.1 ms of 0.5 (its figure sits near the limit, and the p95 of 8 samples is their most);
/// the gradient Normal, since a generator's figure includes compositing its own picture (~0.4 ms there) beside its pass.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Mac only: the budget's calibration on Iris Plus"]
async fn the_budget_table_is_calibrated_on_this_gpu() {
    use klipolo_engine::make::budget::{MachineKey, budget_for, classify};
    use klipolo_engine::make::draft::DraftLayer;
    use klipolo_engine::make::envelope::costly_list;
    use klipolo_engine::platform::gpu::GpuClass;
    use klipolo_model::contrib::{ContribId, builtin};
    use klipolo_model::params::ParamValue;

    let mut h = headless_alone().await;
    let info = h.device().adapter.get_info();
    let os = klipolo_platform::caps::current().os;
    println!("machine: {}", MachineKey::of(&info, &os).as_str());
    println!("trusted for timing: {}", h.device().timestamps_trusted());
    let b = budget_for(GpuClass::of(&info));
    let tmp = tempfile::tempdir().unwrap();
    let built_in = |id: &str, params: &[(&str, f64)]| {
        let params = params
            .iter()
            .map(|(k, v)| (k.to_string(), ParamValue::Number(*v)))
            .collect();
        DraftLayer::builtin(
            &ContribId(id.into()),
            params,
            klipolo_model::project::FormatKind::Landscape,
            tmp.path(),
        )
        .unwrap_or_else(|| panic!("{id} is a built-in shader"))
    };
    let rows = [
        (
            "blur, radius 12 (built-in)",
            built_in(builtin::EFFECT_BLUR, &[("radius", 12.0)]),
        ),
        (
            "vignette (built-in)",
            built_in(builtin::EFFECT_VIGNETTE, &[]),
        ),
        (
            "gradient (built-in)",
            built_in(builtin::GENERATOR_GRADIENT, &[]),
        ),
        ("waves (sample)", draft(draft_def(), WAVES_BODY)),
        ("sparkle (sample)", draft(sparkle_def(), SPARKLE_BODY)),
    ];
    println!(
        "| shader | preview p50 ms | preview p95 ms | 1080p p95 ms | batches (frames, single/batch ms) | resolved \
         | decided by | class | runs at |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    let mut advice = Vec::new();
    let mut meter = GpuMeter::new();
    for (name, layer) in &rows {
        let facts = layer.facts();
        let costly = costly_list(
            &layer.def,
            facts.map_or(&[][..], |f| f.costly_params.as_slice()),
        );
        meter.begin();
        // Each row at its own settings: the measurement's base is the defaults under these.
        let spec = MeasureSpec {
            params: layer.params.clone(),
            ..MeasureSpec::default()
        };
        let m = measure(&mut meter, &mut h, layer, &spec, &costly)
            .await
            .unwrap();
        let (class, runs, said) = classify(&m, facts, &layer.def, &b);
        let (batches, resolved) = fenced_columns(m.fenced.as_ref());
        println!(
            "| {name} | {:.3} | {:.3} | {:.3} | {batches} | {resolved} | {:?} | {class:?} | {runs:?} |",
            m.preview.p50, m.preview.p95, m.p1080.p95, m.decided
        );
        advice.push((name, meter.cold(), said));
    }
    for (name, cold, said) in advice {
        println!("{name}: first frames {cold:?}; {}", said.join(" "));
    }
}

/// Run by hand on a Mac after a Mac build: every shader preset, measured as the worker measures a draft on this GPU
/// (`DraftLayer::builtin` at its defaults, then each costly setting at its costly end, as the guard reads the preset
/// under an agent's id; a built-in has no facts of its own) and classed by `classify` (time and memory, each kind's own
/// share), meets the class it claims (`SHADER_CLAIMED`). A preset over its claim is reworked before it merges; a claim
/// changes only with a note saying why.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Mac only: the shader presets' classes on Iris Plus"]
async fn every_shader_preset_meets_its_claimed_class_on_this_gpu() {
    use klipolo_engine::make::budget::{MachineKey, budget_for, classify};
    use klipolo_engine::make::draft::DraftLayer;
    use klipolo_engine::make::envelope::costly_list;
    use klipolo_engine::platform::gpu::GpuClass;
    use klipolo_engine::registry::guard;
    use klipolo_engine::registry::presets::{SHADER_BODIES, SHADER_CLAIMED};
    use klipolo_model::contrib::ContribId;
    use klipolo_model::project::FormatKind;

    let mut h = headless_alone().await;
    let info = h.device().adapter.get_info();
    let os = klipolo_platform::caps::current().os;
    println!("machine: {}", MachineKey::of(&info, &os).as_str());
    println!("trusted for timing: {}", h.device().timestamps_trusted());
    let b = budget_for(GpuClass::of(&info));
    let tmp = tempfile::tempdir().unwrap();
    // With the batches' columns, so a preset over its claim shows whether its batches settled it.
    println!(
        "| preset | claims | preview p50 ms | preview p95 ms | 1080p p95 ms | batches (frames, single/batch ms) | \
         resolved | peak bytes | decided by | class | runs at |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    let mut over = Vec::new();
    let mut advice = Vec::new();
    for ((local, claim), (_, body)) in SHADER_CLAIMED.iter().zip(SHADER_BODIES) {
        let id = ContribId::builtin(local);
        let layer = DraftLayer::builtin(&id, Default::default(), FormatKind::Landscape, tmp.path())
            .unwrap_or_else(|| panic!("{local} is a built-in shader"));
        let mut as_agent = (*layer.def).clone();
        as_agent.id = format!("lib.k3f9q2ma/{local}.v1").as_str().into();
        let facts = guard::check(&as_agent, layer.kind, body)
            .unwrap_or_else(|d| panic!("{local}: {d:?}"))
            .facts;
        let costly = costly_list(&layer.def, &facts.costly_params);
        let mut meter = GpuMeter::new();
        meter.begin();
        let m = measure(&mut meter, &mut h, &layer, &MeasureSpec::default(), &costly)
            .await
            .unwrap();
        let (class, runs, said) = classify(&m, Some(&facts), &layer.def, &b);
        let (batches, resolved) = fenced_columns(m.fenced.as_ref());
        println!(
            "| {local} | {claim:?} | {:.3} | {:.3} | {:.3} | {batches} | {resolved} | {} | {:?} | {class:?} | \
             {runs:?} |",
            m.preview.p50, m.preview.p95, m.p1080.p95, m.bytes.peak, m.decided
        );
        advice.push((local, meter.cold(), said));
        if class > *claim {
            over.push(format!("{local}: {class:?}, claims {claim:?}"));
        }
    }
    for (local, cold, said) in advice {
        println!("{local}: first frames {cold:?}; {}", said.join(" "));
    }
    assert!(over.is_empty(), "presets over their claim: {over:?}");
}

/// The fenced columns of a row: the preview batches' frames and median single frame and batch, and whether they
/// resolved ("-" where the timing is trusted).
fn fenced_columns(fenced: Option<&klipolo_engine::make::budget::Fenced>) -> (String, String) {
    use klipolo_engine::make::measure::Pcts;
    let Some(f) = fenced else {
        return ("-".into(), "-".into());
    };
    let batches = f.preview.as_ref().map_or("-".to_owned(), |b| {
        let fewest = b.samples.iter().map(|s| s.frames).min().unwrap_or(0);
        let most = b.samples.iter().map(|s| s.frames).max().unwrap_or(0);
        let single: Vec<f64> = b.samples.iter().map(|s| s.single_ms).collect();
        let batch: Vec<f64> = b.samples.iter().map(|s| s.batch_ms).collect();
        format!(
            "{fewest}–{most}, {:.3}/{:.3}",
            Pcts::of(&single).p50,
            Pcts::of(&batch).p50
        )
    });
    let steady = f.preview.as_ref().map_or("", |b| {
        if b.baseline_steady {
            "; baseline steady"
        } else {
            "; baseline unsteady"
        }
    });
    // "settled" says whether the batches told Light or not; over Light, they only said it isn't.
    let over_light = f.preview.as_ref().is_some_and(|b| b.over_light);
    let resolved = match (f.resolved, f.held, over_light) {
        (true, _, true) => "yes (over Light)",
        (true, _, false) => "yes (to Light's precision)",
        (false, true, _) => "no (held at Normal)",
        (false, false, _) => "no",
    };
    (batches, format!("{resolved}{steady}"))
}

/// Run by hand on a Mac after a Mac build: a generator whose loop runs 4, 16, 32 and 64 times, measured as the worker
/// measures a draft. Where the timing is fenced, the batches' figures must rise with the work, roughly in proportion
/// (single fenced frames couldn't tell them apart). Prints each figure, class and the batches' frames.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Mac only: fenced batches on Iris Plus"]
async fn fenced_figures_rise_with_the_work() {
    use klipolo_engine::make::budget::{budget_for, classify};
    use klipolo_engine::platform::gpu::GpuClass;
    let mut h = headless_alone().await;
    println!("trusted for timing: {}", h.device().timestamps_trusted());
    let b = budget_for(GpuClass::of(&h.device().adapter.get_info()));
    let mut meter = GpuMeter::new();
    let mut figures = Vec::new();
    for trips in [4, 16, 32, 64] {
        let body = format!(
            "fn effect(uv: vec2<f32>) -> vec4<f32> {{
    var c = 0.0;
    for (var i = 0; i < {trips}; i++) {{ c += sin(f32(i) * 1.7 + uv.x * 13.0) * cos(uv.y * 7.0 + f32(i)); }}
    return vec4<f32>(vec3<f32>(fract(c)), 1.0);
}}"
        );
        let layer = draft(draft_def_with("[]"), &body);
        meter.begin();
        let m = measure(&mut meter, &mut h, &layer, &MeasureSpec::default(), &[])
            .await
            .unwrap();
        let (class, _, _) = classify(&m, layer.facts(), &layer.def, &b);
        let (batches, resolved) = fenced_columns(m.fenced.as_ref());
        println!(
            "{trips} trips: preview p50 {:.3} p95 {:.3} ms, 1080p p95 {:.3} ms; batches {batches}; resolved {resolved}; \
             {class:?}",
            m.preview.p50, m.preview.p95, m.p1080.p95
        );
        figures.push(m.preview.p50);
    }
    assert!(
        figures[3] > figures[0],
        "64 trips read no more than 4: {figures:?}"
    );
}

/// A meter that counts the setting combinations measured: each is stepped from a 16 × 16 frame first.
struct Counting {
    inner: GpuMeter,
    points: usize,
}

impl Meter for Counting {
    fn frame<'a>(
        &'a mut self,
        h: &'a mut klipolo_engine::headless::Headless,
        req: &'a klipolo_engine::make::measure::FrameReq,
    ) -> futures::future::BoxFuture<'a, Result<klipolo_engine::make::measure::Sample, String>> {
        if req.size == (16, 16) {
            self.points += 1;
        }
        self.inner.frame(h, req)
    }

    fn batch<'a>(
        &'a mut self,
        h: &'a mut klipolo_engine::headless::Headless,
        req: &'a klipolo_engine::make::measure::FrameReq,
        frames: u32,
    ) -> futures::future::BoxFuture<'a, Result<klipolo_engine::make::measure::Sample, String>> {
        self.inner.batch(h, req, frames)
    }

    fn warm<'a>(
        &'a mut self,
        h: &'a mut klipolo_engine::headless::Headless,
        req: &'a klipolo_engine::make::measure::FrameReq,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        self.inner.warm(h, req)
    }

    fn begin(&mut self) {
        self.inner.begin();
    }

    fn cold(&self) -> Vec<klipolo_engine::make::measure::Cold> {
        self.inner.cold()
    }

    fn fenced(&self, h: &klipolo_engine::headless::Headless) -> bool {
        self.inner.fenced(h)
    }
}

/// Measures the capped draft whose body runs `main` trips, as a compile is (from a `COMPILE_BUDGET` deadline; in the
/// worker the hardening before it counts too), prints the total and what was measured, and asserts the caps were really
/// measured: no cut combination decided the class, the corner cap reached (its note), the corners, values and probes
/// measured at all (at least the 12 combinations and one probe, counted by their 16 × 16 steps), nothing cut by the
/// deadline (no "ran out" note), and the ranges of `c` and `d` and the choices of `mode`, `x` and `y` in the envelope.
/// The class itself isn't asserted: it is the costliest of the defaults, the corners and values within the envelope and
/// every probe, so a probe at Normal's limit can make it Over.
async fn time_the_capped_draft(name: &str, main: u32, gated_sin: bool) {
    use klipolo_engine::make::budget::{budget_for, classify};
    use klipolo_engine::make::envelope::costly_list;
    use klipolo_engine::make::measure::{COMPILE_BUDGET, Deadline, Decided, measure_noted};
    use klipolo_engine::platform::gpu::GpuClass;
    let mut h = headless_alone().await;
    println!("trusted for timing: {}", h.device().timestamps_trusted());
    let b = budget_for(GpuClass::of(&h.device().adapter.get_info()));
    let layer = draft(draft_def_with(CAP_SETTINGS), &cap_body(main, gated_sin));
    let facts = layer.facts();
    let costly = costly_list(
        &layer.def,
        facts.map_or(&[][..], |f| f.costly_params.as_slice()),
    );
    let mut meter = Counting {
        inner: GpuMeter::new(),
        points: 0,
    };
    meter.begin();
    let started = std::time::Instant::now();
    let deadline = Deadline::new(started, 0, COMPILE_BUDGET);
    let (m, notes) = measure_noted(
        &mut meter,
        &mut h,
        &layer,
        &MeasureSpec::default(),
        &costly,
        deadline,
        &b,
    )
    .await
    .unwrap();
    let total = started.elapsed();
    let (class, _, said) = classify(&m, facts, &layer.def, &b);
    let (batches, resolved) = fenced_columns(m.fenced.as_ref());
    println!(
        "{name}: measured in {:.2} s of the {} s compile budget; {} combinations measured; {class:?} at {:?}; \
         preview p50 {:.3} p95 {:.3} ms; batches {batches}; settled {resolved}",
        total.as_secs_f64(),
        COMPILE_BUDGET.as_secs(),
        meter.points,
        m.decided,
        m.preview.p50,
        m.preview.p95
    );
    println!(
        "{name}: ranges {:?}; allowed {:?}",
        m.envelope.ranges, m.envelope.allowed
    );
    println!("{name}: notes {notes:?}; advice: {}", said.join(" "));
    assert!(
        !notes.iter().any(|n| n.contains("ran out")),
        "{name}: the deadline cut combinations: {notes:?}"
    );
    assert!(
        m.decided != Decided::Unmeasured,
        "{name}: a cut combination decided the class: {:?}",
        m.decided
    );
    assert!(
        notes.iter().any(|n| n.contains("at most 12 corners")),
        "{name}: the corner cap wasn't reached: {notes:?}"
    );
    assert!(
        meter.points > 12,
        "{name}: {} combinations measured (12 and a probe at least)",
        meter.points
    );
    for key in ["c", "d"] {
        assert!(
            m.envelope.ranges.contains_key(key),
            "{name}: no range for {key}: {:?}",
            m.envelope
        );
    }
    for key in ["mode", "x", "y"] {
        assert!(
            m.envelope.allowed.contains_key(key),
            "{name}: no choices for {key}: {:?}",
            m.envelope
        );
    }
    assert!(total < COMPILE_BUDGET, "{name}: {total:?}");
}

/// Run by hand on a Mac after a Mac build: the capped draft with a body near 1.5 ms on Iris Plus
/// (8 trips of the sweep's loop and 2 more at the defaults), where a fenced point's first sample shows it over Light and
/// the rest take short batches.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Mac only: a capped draft's measuring time on Iris Plus"]
async fn a_draft_at_the_setting_caps_fits_the_compile_budget() {
    time_the_capped_draft("a draft at the caps, ~1.5 ms", 8, true).await;
}

/// The same with a light body (2 trips, ~0.65 ms on Iris Plus): within twice its resolution of Light, so every sample
/// takes the full span, the slowest a fenced point gets.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Mac only: a capped light draft's measuring time on Iris Plus"]
async fn a_light_draft_at_the_setting_caps_fits_the_compile_budget() {
    time_the_capped_draft("a light draft at the caps, ~0.65 ms", 2, false).await;
}

/// Run by hand on a Mac after a Mac build: the app's preview (`Interactive`) on the main device draws 300 frames with
/// one checked filter, sampled every 30th frame as the preview samples it. Prints whether the main device has timestamp
/// queries (expected on Iris Plus) and the sampling's own cost a frame (expected under 0.05 ms), measured inside the
/// sampler and as the difference to 300 frames drawn with sampling off.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Mac only: the preview's sampling on Iris Plus"]
async fn the_preview_samples_a_checked_filter_cheaply_on_this_gpu() {
    use klipolo_engine::api::EngineEvent;
    use klipolo_engine::make::budget::Cost;
    use klipolo_engine::render::mode::RenderMode;
    const DIM: &str = "fn effect(uv: vec2<f32>) -> vec4<f32> { return kl_sample_a(uv) * 0.9; }";
    // A Normal verdict on the filter, so it is sampled (built-ins never are).
    let mut g =
        headless_with_verified_filter(DIM, Cost::with_export_ms(1.0), RenderMode::Interactive)
            .await;
    println!("adapter: {}", g.h.device().adapter_name());
    println!(
        "main device timestamp queries: {}",
        if g.h.device().has_timestamps() {
            "on"
        } else {
            "off"
        }
    );
    g.h.prepare_registry();
    g.acknowledge_marks(); // the compile mark
    g.wait_for_pipelines().await;
    g.render_at(0).await;
    g.acknowledge_marks(); // the draw mark
    let started = std::time::Instant::now();
    for n in 0..300u32 {
        g.render_at(secs(f64::from(n) / 30.0)).await;
    }
    let with_ms = started.elapsed().as_secs_f64() * 1000.0 / 300.0;
    let own_ms = g.h.sampler_cost().as_secs_f64() * 1000.0 / 300.0;
    g.h.set_sampling(false);
    let started = std::time::Instant::now();
    for n in 0..300u32 {
        g.render_at(secs(f64::from(n) / 30.0)).await;
    }
    let without_ms = started.elapsed().as_secs_f64() * 1000.0 / 300.0;
    let degraded = g
        .events()
        .iter()
        .filter(|e| matches!(e, EngineEvent::EffectDegraded { .. }))
        .count();
    println!(
        "sampling's own cost: {own_ms:.4} ms a frame (inside the sampler); frames {with_ms:.3} ms sampled, \
         {without_ms:.3} ms not; effects slowed down: {degraded}"
    );
    assert!(own_ms < 0.05, "{own_ms} ms a frame");
}
