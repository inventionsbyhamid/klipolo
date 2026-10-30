//! A checked scene's `Cost` on this computer (the classes). The measurement is the worker's
//! (`measure_noted`: stepped corners, the 50 ms prediction and the 24 ms Over stop, 1080p only within class), with the
//! scene as the draft layer and its costly settings ([`costly`](super::costly)) at their costly end. A table a `repeat`
//! draws a row of is measured at its most rows throughout and never narrowed: an Over there is
//! Over for the whole scene. Then what only a scene has, at the defaults and at the envelope's costly end:
//! - its CPU time a frame: the evaluator over [`CPU_FRAMES`] frames, and the scene drawer's own time (evaluate,
//!   pack, upload, encode) over the frames drawn here, the worse p95 of the two;
//! - its memory as the drawer holds it (`SceneBytes::split`, the one rule for every caller): its own share,
//!   then the extra targets and bytes. Neither is drawn for a scene already Over on time.
//!
//! Scenes never run at half size, so CPU past 300 µs, or memory past Normal's, is Over.

use std::sync::Arc;
use std::time::Instant;

use klipolo_model::contrib::ContribDef;
use klipolo_model::params::{ParamKind, ParamValue, TableRow};
use klipolo_model::project::FormatKind;
use klipolo_model::scene::eval::{SceneFrame, eval_into};
use klipolo_model::scene::instance::SceneInstance;
use klipolo_model::scene::timeline::CompiledScene;

use super::CheckedScene;
use crate::headless::Headless;
use crate::make::budget::{Cost, CostClass, MachineKey, budget_for, classify};
use crate::make::draft::DraftLayer;
use crate::make::envelope::{Envelope, Params};
use crate::make::measure::{Deadline, Measured, Meter, Pcts, measure_noted};
use crate::make::protocol::MeasureSpec;
use crate::make::sizes::preview_size;
use crate::platform::gpu::GpuClass;
use crate::registry::guard::{CostlyParam, End};
use crate::render::scene::SceneSplit;

/// Frames the evaluator's CPU time is taken over (per frame, p95).
pub const CPU_FRAMES: usize = 60;
/// Frames drawn at preview size at each point for the drawer's own time and memory.
const DRAWN_FRAMES: usize = 8;

/// `c`'s cost at `spec`'s settings, and the notes the measurement made (what a cut left out).
pub async fn scene_cost(
    meter: &mut dyn Meter,
    h: &mut Headless,
    c: &CheckedScene,
    spec: &MeasureSpec,
    deadline: Deadline,
) -> Result<Cost, String> {
    let costly = super::costly(&c.scene);
    let base = with_most_rows(&c.def, &costly, &spec.params);
    let format = FormatKind::Landscape;
    let draft = DraftLayer::of_scene(
        &c.def,
        c.scene.clone(),
        c.registry.clone(),
        base.clone(),
        format,
    );
    meter.begin();
    let info = h.device().adapter.get_info();
    let b = budget_for(GpuClass::of(&info));
    let asked = MeasureSpec {
        params: base.clone(),
        ..spec.clone()
    };
    let (mut m, mut advice) =
        measure_noted(meter, h, &draft, &asked, &costly, deadline, &b).await?;
    let mut points = vec![base.clone()];
    let worst = costly_end(&c.def, &costly, &m.envelope, &base);
    if worst != base {
        points.push(worst);
    }
    let mut cpu = Pcts::default();
    let mut held = None;
    for p in &points {
        cpu = cpu.worse(eval_cpu(&c.scene, p, draft.seconds, CPU_FRAMES));
        if m.class != CostClass::Over {
            let (drawer, split) = drawn(h, &draft.at(p.clone())).await?;
            cpu = cpu.worse(drawer);
            held = Some(held.map_or(split, |h: SceneSplit| most(h, split)));
        }
    }
    m.cpu_us = Some(meter.scene_cpu(cpu));
    if let Some(split) = held {
        own_memory(&mut m, split);
    }
    let (class, runs_at, more) = classify(&m, None, &c.def, &b);
    advice.extend(more);
    let machine = MachineKey::of(&info, &klipolo_platform::caps::current().os);
    Ok(Cost::of(&m, (class, runs_at, advice), &c.def, &machine, &b))
}

/// `values` over the defaults, with every costly table at its most rows (its default rows over again, in order).
fn with_most_rows(def: &ContribDef, costly: &[CostlyParam], values: &Params) -> Params {
    let mut out = values.clone();
    for c in costly {
        let Some(spec) = def.param(&c.key) else {
            continue;
        };
        if let ParamKind::Table {
            columns,
            max_rows,
            default,
        } = &spec.kind
        {
            let most = (*max_rows).min(klipolo_model::params::MAX_TABLE_ROWS) as usize;
            let rows: Vec<TableRow> = (0..most)
                .map(|i| {
                    default
                        .get(i % default.len().max(1))
                        .cloned()
                        .unwrap_or(TableRow {
                            label: String::new(),
                            values: vec![0.0; columns.len()],
                        })
                })
                .collect();
            out.insert(c.key.clone(), ParamValue::Rows(rows));
        }
    }
    out
}

/// `base` with every costly number at its envelope's costly end: the most the drawn layer may ask of it.
fn costly_end(def: &ContribDef, costly: &[CostlyParam], env: &Envelope, base: &Params) -> Params {
    let mut out = base.clone();
    for c in costly {
        let (Some(&(lo, hi)), Some(spec)) = (env.ranges.get(&c.key), def.param(&c.key)) else {
            continue;
        };
        let x = if c.costly_end == End::Min { lo } else { hi };
        let x = match spec.kind {
            ParamKind::Int { .. } => x.floor(),
            _ => x,
        };
        out.insert(c.key.clone(), ParamValue::Number(x));
    }
    out
}

/// The evaluator's time a frame (µs) at `params`, over `n` frames spread across `seconds`, once warm.
pub(crate) fn eval_cpu(
    scene: &Arc<CompiledScene>,
    params: &Params,
    seconds: f64,
    n: usize,
) -> Pcts {
    let inst = SceneInstance::new(scene, params);
    let mut frame = SceneFrame::for_instance(&inst);
    eval_into(&inst, 0.0, &mut frame);
    let us: Vec<f64> = (0..n.max(1))
        .map(|i| {
            let t = (i as f64 + 0.5) / n.max(1) as f64 * seconds;
            let started = Instant::now();
            eval_into(&inst, t, &mut frame);
            started.elapsed().as_secs_f64() * 1e6
        })
        .collect();
    Pcts::of(&us)
}

/// `layer` drawn at preview size (one untimed frame first): the drawer's own time a frame, and the most its memory
/// split reached.
async fn drawn(h: &mut Headless, layer: &DraftLayer) -> Result<(Pcts, SceneSplit), String> {
    layer.prepare(h, true, false);
    let project = Arc::new(layer.project());
    let size = preview_size(layer.format);
    let layer_bytes = u64::from(size.0) * u64::from(size.1) * 4;
    h.render_timed(project.clone(), layer.middle(), layer.format, size)
        .await?;
    let (mut us, mut split) = (Vec::with_capacity(DRAWN_FRAMES), SceneSplit::default());
    for at in layer.times(DRAWN_FRAMES) {
        h.render_timed(project.clone(), at, layer.format, size)
            .await?;
        let stats = h.scene_stats();
        us.push(stats.cpu_us_last);
        split = most(split, stats.bytes.split(layer_bytes));
    }
    Ok((Pcts::of(&us), split))
}

/// The larger of two splits, part by part.
fn most(a: SceneSplit, b: SceneSplit) -> SceneSplit {
    SceneSplit {
        own: a.own.max(b.own),
        extra_targets: a.extra_targets.max(b.extra_targets),
        extra_bytes: a.extra_bytes.max(b.extra_bytes),
    }
}

/// `m`'s memory as the scene drawer holds it: its own share and its extras, with the pool's peak kept.
fn own_memory(m: &mut Measured, split: SceneSplit) {
    m.bytes.own = split.own;
    m.bytes.extra_targets = split.extra_targets;
    m.bytes.extra_bytes = split.extra_bytes;
    m.bytes.peak = m.bytes.peak.max(split.own);
}
