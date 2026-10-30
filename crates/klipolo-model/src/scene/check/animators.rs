//! A text node's animators: the counter (its values within ±1e15), the typewriter, scrambles and holds, each timed
//! inside the scene; and the characters they can show beyond the text.

use super::super::problems::{at, join, Problems};
use super::super::{Node, SceneLimits, Val};
use super::values::{param_range, seconds, text, text_len, val, Want, NUMBER_CHARS};
use super::{Repeat, Scope};
use crate::ease_names;

/// The farthest a counter counts either way, so its text stays short.
const MAX_COUNT: f64 = 1e15;

/// Checks the counter, typewriter, scramble and holds; the characters they add.
pub(super) fn animators(
    s: &Scope,
    out: &mut Problems,
    n: &Node,
    path: &str,
    rep: Option<Repeat>,
) -> usize {
    let p = &n.props;
    let mut chars = 0usize;
    if let Some(count) = &p.count {
        let count_path = join(path, "count");
        // Within ±1e15, so it formats into a bounded buffer: literals, and a bound setting's whole range.
        for (v, key) in [(&count.from, "from"), (&count.to, "to")] {
            let value_path = join(&count_path, key);
            let most = val(s, out, v, Want::Number, &value_path, rep);
            let bound = match v {
                Val::Str(bound) => param_range(s, bound),
                Val::Num(_) => None,
            };
            let (lo, hi) = bound.unwrap_or((most.unwrap_or(0.0), most.unwrap_or(0.0)));
            if !(lo.abs() <= MAX_COUNT && hi.abs() <= MAX_COUNT) {
                out.push(
                    value_path,
                    "scene.value",
                    "A counter counts within ±1e15 (a thousand trillion).",
                );
            }
        }
        if count.decimals.is_some_and(|d| d > 6) {
            out.push(
                join(&count_path, "decimals"),
                "scene.value",
                "A counter shows 0 to 6 decimals.",
            );
        }
        let affix = |a: &Option<String>| a.as_ref().map_or(0, |t| t.chars().count());
        if affix(&count.prefix) > 16 || affix(&count.suffix) > 16 {
            out.push(
                count_path.clone(),
                "scene.value",
                "A counter's prefix and suffix are up to 16 characters.",
            );
        }
        chars = chars.saturating_add(NUMBER_CHARS + affix(&count.prefix) + affix(&count.suffix));
        animator(
            s,
            out,
            count.at,
            count.dur,
            count.ease.as_deref(),
            &count_path,
        );
    }
    if let Some(reveal) = &p.reveal {
        animator(
            s,
            out,
            reveal.at,
            reveal.dur,
            reveal.ease.as_deref(),
            &join(path, "reveal"),
        );
    }
    if let Some(scramble) = &p.scramble {
        let scramble_path = join(path, "scramble");
        if scramble.chars.as_ref().is_some_and(|c| {
            !(1..=64).contains(&c.chars().count()) || c.chars().any(char::is_control)
        }) {
            out.push(
                join(&scramble_path, "chars"),
                "scene.value",
                "A scramble draws from 1 to 64 characters.",
            );
        }
        animator(s, out, scramble.at, scramble.dur, None, &scramble_path);
    }
    if let Some(holds) = &p.holds {
        let holds_path = join(path, "holds");
        if holds.len() > SceneLimits::MAX_KEYS {
            out.push(
                holds_path.clone(),
                "scene.tooManyKeys",
                format!("A text has at most {} holds.", SceneLimits::MAX_KEYS),
            );
        }
        let mut last = f64::NEG_INFINITY;
        for (i, hold) in holds.iter().enumerate() {
            let hold_path = at(&holds_path, i);
            seconds(s, out, Some(hold.t), &join(&hold_path, "t"));
            if hold.t < last {
                out.push(
                    join(&hold_path, "t"),
                    "scene.keys",
                    "Holds are in time order.",
                );
            }
            last = hold.t;
            text(
                s,
                out,
                &hold.text,
                Want::Text,
                &join(&hold_path, "text"),
                rep,
            );
            chars = chars.saturating_add(text_len(s, &hold.text, p.case));
        }
    }
    chars
}

/// A text animator's start, length and ease, inside the scene.
fn animator(
    s: &Scope,
    out: &mut Problems,
    at_s: Option<f64>,
    dur: Option<f64>,
    ease: Option<&str>,
    path: &str,
) {
    seconds(s, out, at_s, &join(path, "at"));
    seconds(s, out, dur, &join(path, "dur"));
    if at_s.unwrap_or(0.0) + dur.unwrap_or(0.0) > s.scene.duration {
        out.push(path, "scene.value", "It starts and ends inside the scene.");
    }
    if let Some(e) = ease {
        if let Err(why) = ease_names::read(e) {
            out.push(join(path, "ease"), "scene.ease", why);
        }
    }
}
