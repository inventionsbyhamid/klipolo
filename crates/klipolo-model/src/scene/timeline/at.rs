//! GSAP's position parameter as data, read once at compile (the same grammar as the check's `check::timing`) and placed
//! wherever the members a stagger spreads over are known: at compile with the check's counts, then again in each
//! instance on the texts actually laid out (the evaluator places tweens on the laid-out texts).

use super::super::check::is_name;
use super::super::{Tween, Val};

/// Where a tween starts, before the timeline is known.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum At {
    Abs(f64),
    /// The timeline's end so far (`"+=s"`, `"-=s"`, and the default).
    End(f64),
    /// The previous tween's start (`"<"`).
    PrevStart(f64),
    /// The previous tween's end (`">"`).
    PrevEnd(f64),
    /// The exit segment's start (`"out"`).
    Out(f64),
    /// A label's time, by its slot.
    Label(usize, f64),
}

/// The labels defined so far, by name; a name defined again takes the later time, as the check's map does.
#[derive(Default)]
pub(crate) struct Labels {
    pub names: Vec<String>,
}

impl Labels {
    fn slot(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    /// The slot a tween's label writes, if it is a label at all (a name, not `out`).
    pub fn define(&mut self, label: &str) -> Option<usize> {
        if !is_name(label) || label == "out" {
            return None;
        }
        Some(self.slot(label).unwrap_or_else(|| {
            self.names.push(label.to_owned());
            self.names.len() - 1
        }))
    }
}

/// Reads a tween's `at` with the labels defined before it; None where the check refuses it.
pub(crate) fn parse(at: Option<&Val>, labels: &Labels, has_out: bool) -> Option<At> {
    let text = match at {
        None => return Some(At::End(0.0)),
        Some(Val::Num(n)) => return Some(At::Abs(*n)),
        Some(Val::Str(text)) => text.trim(),
    };
    if let Some(slot) = labels.slot(text) {
        return Some(At::Label(slot, 0.0));
    }
    if let Some(t) = text.parse::<f64>().ok().filter(|t| t.is_finite()) {
        return Some(At::Abs(t));
    }
    // The longest label the text starts with, then a signed offset ("my-intro+=0.2").
    let labelled = labels
        .names
        .iter()
        .enumerate()
        .filter(|(_, label)| text.starts_with(label.as_str()))
        .max_by_key(|(_, label)| label.len())
        .and_then(|(slot, label)| {
            let rest = text.get(label.len()..)?.trim_start();
            rest.starts_with(['+', '-']).then_some(())?;
            Some(At::Label(slot, offset(rest)?))
        });
    if labelled.is_some() {
        return labelled;
    }
    let cut = if let Some(rest) = text.strip_prefix(['<', '>']) {
        text.len() - rest.len()
    } else {
        text.find(['+', '-']).unwrap_or(text.len())
    };
    let (anchor, rest) = (text.get(..cut)?.trim(), text.get(cut..)?);
    let off = offset(rest)?;
    Some(match anchor {
        "" => At::End(off),
        "<" => At::PrevStart(off),
        ">" => At::PrevEnd(off),
        "out" if has_out => At::Out(off),
        label => At::Label(labels.slot(label)?, off),
    })
}

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

/// What placing reads of a tween besides its position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Timing {
    pub at: At,
    pub dur: f64,
    /// −1 repeats until the scene ends.
    pub repeat: i32,
    pub delay: f64,
    /// The label slot this tween's start defines.
    pub label: Option<usize>,
}

impl Timing {
    pub fn of(t: &Tween, at: At, label: Option<usize>) -> Self {
        let dur = t.dur.unwrap_or(if t.set.is_some() {
            0.0
        } else {
            super::super::DEFAULT_TWEEN_DUR
        });
        Self {
            at,
            dur: if dur.is_finite() {
                dur.clamp(0.0, 600.0)
            } else {
                0.0
            },
            repeat: t.repeat.unwrap_or(0).clamp(-1, 1000),
            delay: t
                .repeat_delay
                .filter(|d| d.is_finite())
                .map_or(0.0, |d| d.clamp(0.0, 600.0)),
            label,
        }
    }
}

/// The timeline so far while placing.
pub(crate) struct Clock {
    prev: Option<(f64, f64)>,
    end: f64,
    labels: Vec<f64>,
}

impl Clock {
    pub fn new(labels: usize) -> Self {
        Self {
            prev: None,
            end: 0.0,
            labels: vec![0.0; labels],
        }
    }

    /// Places one tween whose stagger spreads `spread` seconds: its start and end in a scene `duration` long, with
    /// the exit segment starting at `out`. The start is held inside the scene (the check refuses one outside it; a
    /// smaller laid-out text only moves tweens earlier).
    pub fn place(&mut self, t: &Timing, spread: f64, duration: f64, out: f64) -> (f64, f64) {
        let start = match t.at {
            At::Abs(v) => v,
            At::End(off) => self.end + off,
            At::PrevStart(off) => self.prev.map_or(0.0, |p| p.0) + off,
            At::PrevEnd(off) => self.prev.map_or(0.0, |p| p.1) + off,
            At::Out(off) => out + off,
            At::Label(slot, off) => self.labels.get(slot).copied().unwrap_or(0.0) + off,
        };
        let start = if start.is_finite() {
            start.clamp(0.0, duration.max(0.0))
        } else {
            0.0
        };
        let end = if t.repeat < 0 {
            duration.max(start)
        } else {
            let runs = f64::from(t.repeat) + 1.0;
            start + spread + runs * t.dur + (runs - 1.0) * t.delay
        };
        if let Some(slot) = t.label {
            if let Some(cell) = self.labels.get_mut(slot) {
                *cell = start;
            }
        }
        self.prev = Some((start, end));
        if end.is_finite() {
            self.end = self.end.max(end);
        }
        (start, end)
    }
}
