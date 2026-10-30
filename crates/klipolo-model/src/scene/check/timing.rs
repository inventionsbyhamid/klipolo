//! When things happen: GSAP's position parameter (`at`) read as data, labels, and how far a stagger spreads.

use std::collections::BTreeMap;

use super::super::problems::{quote, Problems};
use super::super::{StaggerArg, Tween, Val};
use super::timeline::Span;
use super::Scope;

/// What the timeline knows so far: the previous tween's start and end, the timeline's end, and labels.
#[derive(Default)]
pub(super) struct Clock {
    pub prev: Option<Span>,
    pub end: f64,
    pub labels: BTreeMap<String, f64>,
}

/// A stagger's each or amount: 0 or more seconds, inside the scene.
pub(super) fn stagger(s: &Scope, out: &mut Problems, st: &StaggerArg, path: &str) {
    let (each, amount) = match *st {
        StaggerArg::Each(e) => (Some(e), None),
        StaggerArg::Spec { each, amount, .. } => (each, amount),
    };
    let ok = [each, amount]
        .iter()
        .flatten()
        .all(|v| v.is_finite() && (0.0..=s.scene.duration).contains(v));
    if !ok || (each.is_some() && amount.is_some()) {
        out.push(
            path,
            "scene.value",
            "A stagger is seconds between units, or {\"each\": s} or {\"amount\": s} (one of them), inside the scene.",
        );
    }
}

/// How long a stagger spreads `units` targets: each × (units − 1), or the amount.
pub(super) fn stagger_total(st: &StaggerArg, units: usize) -> f64 {
    let gaps = units.saturating_sub(1) as f64;
    let total = match *st {
        StaggerArg::Each(e) => e * gaps,
        StaggerArg::Spec { each: Some(e), .. } => e * gaps,
        StaggerArg::Spec {
            amount: Some(a), ..
        } => a,
        StaggerArg::Spec { .. } => 0.0,
    };
    if total.is_finite() {
        total.max(0.0)
    } else {
        0.0
    }
}

/// The seconds between units a stagger gives, for `units` of them.
pub(super) fn stagger_each(st: &StaggerArg, units: usize) -> f64 {
    let gaps = units.saturating_sub(1).max(1) as f64;
    let each = match *st {
        StaggerArg::Each(e) => e,
        StaggerArg::Spec { each: Some(e), .. } => e,
        StaggerArg::Spec {
            amount: Some(a), ..
        } => a / gaps,
        StaggerArg::Spec { .. } => 0.0,
    };
    if each.is_finite() {
        each.max(0.0)
    } else {
        0.0
    }
}

/// When a tween starts: GSAP's position parameter, read as data.
pub(super) fn start_time(
    s: &Scope,
    out: &mut Problems,
    clock: &Clock,
    at_v: Option<&Val>,
    path: &str,
) -> Option<f64> {
    let duration = s.scene.duration;
    let time = match at_v {
        None => Ok(clock.end),
        Some(Val::Num(n)) => Ok(*n),
        Some(Val::Str(text)) => position(s, clock, text.trim()),
    };
    match time {
        Ok(t) if (0.0..=duration).contains(&t) => Some(t),
        Ok(t) => {
            out.push(
                path,
                "scene.at",
                format!("This tween would start at {t} s, outside the scene (0 to {duration} s)."),
            );
            None
        }
        Err(why) => {
            out.push(path, "scene.at", why);
            None
        }
    }
}

/// `<`, `>`, `+=s`, `-=s`, `label`, `label+=s`, `out`, `out+s`, each with an optional offset.
fn position(s: &Scope, clock: &Clock, text: &str) -> Result<f64, String> {
    if let Some(t) = clock.labels.get(text) {
        return Ok(*t);
    }
    if let Some(t) = text.parse::<f64>().ok().filter(|t| t.is_finite()) {
        return Ok(t);
    }
    // A label with a dash ("my-intro+=0.2"): the longest label the text starts with, then its offset, which starts
    // with a sign ("intro2" is no label, not intro + 2 s).
    let labelled = clock
        .labels
        .iter()
        .filter(|(label, _)| text.starts_with(label.as_str()))
        .max_by_key(|(label, _)| label.len())
        .and_then(|(label, t)| {
            let rest = text.get(label.len()..)?.trim_start();
            rest.starts_with(['+', '-']).then_some(())?;
            Some(t + offset(rest)?)
        });
    if let Some(t) = labelled {
        return Ok(t);
    }
    let shown = quote(text);
    let (anchor, offset) = split_offset(text).ok_or_else(|| {
        format!("\"{shown}\" isn't a position: write seconds, \"<\", \">\", \"+=0.2\", \"-=0.2\", a label or \"out\".")
    })?;
    let base = match anchor {
        "" => clock.end,
        "<" => clock.prev.map_or(0.0, |p| p.start),
        ">" => clock.prev.map_or(0.0, |p| p.end),
        "out" => match s.scene.segments {
            Some(seg) => s.scene.duration - seg.out,
            None => {
                return Err(
                    "\"out\" is the exit segment's start: give the scene \"segments\": {\"in\": s, \"out\": s}."
                        .into(),
                )
            }
        },
        label => match clock.labels.get(label) {
            Some(t) => *t,
            None => return Err(format!("No earlier tween defines the label \"{}\".", quote(label))),
        },
    };
    Ok(base + offset)
}

/// `anchor` and its offset: `"<0.2"`, `"<+=0.2"`, `"out-0.3"`, `"intro+=1"`, `"+=0.5"`.
fn split_offset(text: &str) -> Option<(&str, f64)> {
    let cut = if let Some(rest) = text.strip_prefix(['<', '>']) {
        text.len() - rest.len()
    } else {
        text.find(['+', '-']).unwrap_or(text.len())
    };
    let (anchor, rest) = (text.get(..cut)?, text.get(cut..)?);
    Some((anchor.trim(), offset(rest)?))
}

/// An offset after an anchor: nothing, `+=s`, `-=s`, `+s`, `-s` or `s`.
fn offset(rest: &str) -> Option<f64> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Some(0.0);
    }
    let (sign, number) = if let Some(n) = rest.strip_prefix("+=") {
        (1.0, n)
    } else if let Some(n) = rest.strip_prefix("-=") {
        (-1.0, n)
    } else if let Some(n) = rest.strip_prefix('+') {
        (1.0, n)
    } else if let Some(n) = rest.strip_prefix('-') {
        (-1.0, n)
    } else {
        (1.0, rest)
    };
    let v: f64 = number.trim().parse().ok().filter(|v: &f64| v.is_finite())?;
    Some(sign * v)
}

/// Yoyo with an odd number of repeats runs its last cycle backwards, so it ends on its start values.
pub(super) fn odd_yoyo(t: &Tween) -> bool {
    t.yoyo == Some(true) && t.repeat.is_some_and(|r| r > 0 && r % 2 == 1)
}
