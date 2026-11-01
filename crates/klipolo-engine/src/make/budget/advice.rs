//! What the agent is told about a cost (English, agent-facing): how it runs, why it is over a class
//! (quoting the figure that decided it), the loop that costs most or the setting that bounds it, what
//! would let it run at half size, and the memory or CPU that raised it. Every Over says why, and no fix is promised
//! that can't work.

use klipolo_model::contrib::{ContribDef, ContribKind};

use super::{Budget, CostClass, OVER_AT_PREVIEW_MS, RunsAt};
use crate::make::measure::{Decided, Measured};
use crate::registry::guard::{LoopFact, StaticFacts, UNKNOWN_LOOP_BOUND};

mod fenced;
mod narrowed;
use narrowed::narrowed;

/// What `classify` found, for the advice.
pub(super) struct Verdict {
    pub class: CostClass,
    pub by_time: CostClass,
    pub by_memory: CostClass,
    pub by_cpu: CostClass,
    pub may_halve: bool,
    /// Heavy, but the definition may not run at half size, so Over.
    pub raised_by_i2: bool,
    pub runs_at: RunsAt,
}

/// Why a Heavy cost is Over for a definition that may not run at half size.
const HEAVY_NEEDS: &str = "Heavy needs a definition that may run at half size (`resolution: half` for a generator, a \
                           `downsample` for a filter)";

/// A timeline's frames a second, for a stepped generator's average.
const TIMELINE_FPS: f64 = 30.0;

pub(super) fn advise(
    m: &Measured,
    facts: Option<&StaticFacts>,
    def: &ContribDef,
    b: &Budget,
    v: &Verdict,
) -> Vec<String> {
    let mut advice = Vec::new();
    // The decided figure, when one was measured rather than where a stop ended it.
    let measured = match m.decided {
        Decided::Preview(ms) | Decided::Half(ms) if ms < OVER_AT_PREVIEW_MS => Some(ms),
        _ => None,
    };
    if v.class == CostClass::Heavy {
        advice.push(match m.decided {
            Decided::Half(ms) if ms < OVER_AT_PREVIEW_MS => format!(
                "Runs at half size: {ms:.1} ms a frame there, within Heavy's {:.1} ms.",
                b.heavy_ms
            ),
            _ => "Runs at half size, where Heavy allows the extra memory it uses.".into(),
        });
    }
    if v.runs_at.cached {
        advice.push(
            "Drawn once for each set of settings and size, then reused: it doesn't animate.".into(),
        );
    }
    if let (true, Some(fps), Some(ms)) = (v.runs_at.stepped, def.fps, measured) {
        let average = ms * f64::from(fps).min(TIMELINE_FPS) / TIMELINE_FPS;
        advice.push(format!(
            "Drawn {fps} times a second (`fps`): about {average:.2} ms a frame on average in a 30 fps video."
        ));
    }
    if v.by_time == CostClass::Over {
        advice.push(over_advice(m.decided, v.may_halve, b));
        advice.extend(half_size_advice(def, m, b, v));
    }
    if v.raised_by_i2 {
        advice.push(format!("{HEAVY_NEEDS}: without one, a Heavy cost is Over."));
    }
    // The loop advice also where a stop ended it: then the figure is a floor ("at least").
    let figure = match m.decided {
        Decided::Preview(ms) | Decided::Half(ms) => Some(ms.min(OVER_AT_PREVIEW_MS)),
        Decided::Unmeasured => None,
    };
    if let (Some(ms), Some(facts)) = (figure, facts) {
        advice.extend(loop_advice(ms, facts, m, def, b, v));
    }
    advice.extend(narrowed(m, def));
    if v.by_memory > v.by_time {
        let mib = m.bytes.extra_bytes as f64 / f64::from(1 << 20);
        let heavy = heavy_allows(v.may_halve);
        advice.push(format!(
            "{} extra layer-sized targets and {mib:.1} MiB beyond its own memory make it {}: Light allows none, \
             Normal one and {} MiB; {heavy} two and {} MiB.",
            m.bytes.extra_targets,
            name(v.by_memory),
            b.normal_extra_bytes >> 20,
            b.heavy_extra_bytes >> 20
        ));
    }
    if let Some(cpu) = m.cpu_us
        && v.by_cpu > v.by_time.max(v.by_memory)
    {
        // As the memory line: Heavy only with a definition that may run at half size.
        advice.push(format!(
            "{:.0} µs of CPU a frame makes it {}: Light allows {:.0} µs, Normal {:.0} µs; {} {:.0} µs.",
            cpu.p95,
            name(v.by_cpu),
            b.light_cpu_us,
            b.normal_cpu_us,
            heavy_allows(v.may_halve),
            b.heavy_cpu_us
        ));
    }
    advice.extend(fenced::fenced(m));
    advice
}

/// Why it is Over, quoting the figure that decided it (Over's threshold for one the time cut). A figure at
/// or past `OVER_AT_PREVIEW_MS` is where a stop ended it, so it reads "over 24.0 ms".
fn over_advice(decided: Decided, may_halve: bool, b: &Budget) -> String {
    let cheaper =
        "make it cheaper (fewer loop iterations or samples), or narrow its settings' ranges";
    let figure = |ms: f64| {
        if ms >= OVER_AT_PREVIEW_MS {
            format!("Over {OVER_AT_PREVIEW_MS:.1} ms")
        } else {
            format!("{ms:.1} ms")
        }
    };
    match decided {
        Decided::Half(ms) => format!(
            "{} a frame at half size against {:.1} ms allowed: {cheaper}.",
            figure(ms),
            b.heavy_ms
        ),
        Decided::Preview(ms) => format!(
            "{} a frame at preview size against {:.1} ms allowed: {cheaper}.",
            figure(ms),
            b.limit_ms(CostClass::Over, may_halve)
        ),
        Decided::Unmeasured => {
            "Some of its settings weren't measured (the note says why), so it counts as Over: make \
             it cheaper, or give it fewer costly settings, so the checker measures them all."
                .into()
        }
    }
}

/// For a definition that may not run at half size but would fit there (a quarter of the pixels within Heavy's time
/// and memory): what to declare. A generator allows `resolution: half`; a filter declares a `downsample`.
fn half_size_advice(def: &ContribDef, m: &Measured, b: &Budget, v: &Verdict) -> Option<String> {
    let Decided::Preview(ms) = m.decided else {
        return None;
    };
    let quarter = ms / 4.0;
    let fits = !v.may_halve
        && ms < OVER_AT_PREVIEW_MS
        && quarter <= b.heavy_ms
        && m.bytes.extra_targets <= b.heavy_extra_targets
        && m.bytes.extra_bytes <= b.heavy_extra_bytes;
    if !fits {
        return None;
    }
    match def.kind {
        ContribKind::Generator => Some(format!(
            "Or allow `resolution: half`: at half size it draws a quarter of the pixels, about {quarter:.1} ms, \
             within Heavy's {:.1} ms.",
            b.heavy_ms
        )),
        ContribKind::Effect => Some(format!(
            "Or, if one of its settings makes the result smooth (a blur radius), declare a `downsample` on it: then it \
             may run at half size, about {quarter:.1} ms, within Heavy's {:.1} ms.",
            b.heavy_ms
        )),
        _ => None,
    }
}

/// What `may_halve` lets Heavy allow, for the memory and CPU lines: Heavy at half size, or what Heavy needs first.
fn heavy_allows(may_halve: bool) -> String {
    if may_halve {
        "Heavy (at half size)".to_owned()
    } else {
        format!("{HEAVY_NEEDS} and then allows")
    }
}

/// How far over the next class down it is, naming the loop that costs most (the one with the most trips) or the
/// setting that bounds it. A definition that may run at half size is held to Heavy there: by its half-size figure, or,
/// where a stop at full size ended it, by a quarter of that figure (`half_size_advice`'s estimate). Only a
/// generator or a filter that may not yet run at half size is told it could.
fn loop_advice(
    ms: f64,
    facts: &StaticFacts,
    m: &Measured,
    def: &ContribDef,
    b: &Budget,
    v: &Verdict,
) -> Option<String> {
    let (over, limit, estimate) = match v.by_time {
        CostClass::Normal => ("Light", b.light_ms, ms),
        CostClass::Over if matches!(m.decided, Decided::Half(_)) => ("Heavy", b.heavy_ms, ms),
        CostClass::Over if v.may_halve => ("Heavy at half size", b.heavy_ms, ms / 4.0),
        CostClass::Over => ("Normal", b.normal_ms, ms),
        _ => return None,
    };
    let ratio = estimate / limit;
    let worst: &LoopFact = facts
        .loops
        .iter()
        .max_by_key(|l| l.max_trips.unwrap_or(UNKNOWN_LOOP_BOUND))?;
    // Where a stop ended it, the figure is only a floor: then the loop is named even at about the limit (a quarter of
    // the stop is Heavy's own 6 ms), with no factor to lower it by.
    let stop = ms >= OVER_AT_PREVIEW_MS;
    let floor = stop && ratio <= 1.0;
    if ratio <= 1.0 && !floor {
        return None;
    }
    let (far, or_fewer) = if floor {
        (String::new(), "")
    } else if stop {
        (format!("At least {ratio:.0}×"), " or fewer")
    } else {
        (format!("{ratio:.1}×"), "")
    };
    let to = |trips: u32| {
        if floor {
            String::new()
        } else {
            format!(" to {}{or_fewer}", lowered(trips, ratio))
        }
    };
    let line = worst.line;
    let half = match def.kind {
        ContribKind::Generator | ContribKind::Effect if !v.may_halve => {
            ", or let it run at half size"
        }
        _ => "",
    };
    let what = match (&worst.bound_param, worst.max_trips) {
        // Fewer than one trip would be needed: lowering this loop alone can't do it.
        (key, Some(trips)) if f64::from(trips) / ratio < 1.0 => {
            let by = key
                .as_ref()
                .map_or(String::new(), |k| format!(" (as often as `{k}` says)"));
            format!(
                "the loop at line {line} runs up to {trips} times{by}, and lowering it alone isn't enough: do less \
                 work a trip{half}"
            )
        }
        (Some(key), Some(trips)) => format!(
            "the loop at line {line} runs as often as `{key}` says (up to {trips} times): lower its maximum{}",
            to(trips)
        ),
        (Some(key), None) => {
            format!("the loop at line {line} runs as often as `{key}` says: lower its maximum")
        }
        (None, Some(trips)) => format!(
            "the loop at line {line} runs up to {trips} times: lower it{}",
            to(trips)
        ),
        (None, None) => format!(
            "the loop at line {line} has no bound the checker can read, so it counts as {UNKNOWN_LOOP_BOUND} times: \
             give it a constant bound"
        ),
    };
    Some(if floor {
        // One colon at most: the floor, then the loop as its own sentence.
        let mut loop_line = what;
        if let Some(first) = loop_line.get(..1) {
            let upper = first.to_uppercase();
            loop_line.replace_range(..1, &upper);
        }
        format!("At half size it is at least Heavy's {limit:.1} ms. {loop_line}.")
    } else {
        format!("{far} over {over}: {what}.")
    })
}

/// The trips that would fit, at least one (`loop_advice` checks that one is enough first).
fn lowered(trips: u32, ratio: f64) -> u32 {
    ((f64::from(trips) / ratio).floor() as u32).max(1)
}

fn name(class: CostClass) -> &'static str {
    match class {
        CostClass::Light => "Light",
        CostClass::Normal => "Normal",
        CostClass::Heavy => "Heavy",
        CostClass::Over => "Over",
    }
}

#[cfg(test)]
mod tests;
