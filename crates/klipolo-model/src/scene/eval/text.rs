//! A text node's animators at a moment: holds pick a text, the typewriter's progress, a scramble of the shown text and
//! the counter's number, written into the node's fixed buffer. Returns the characters the node shows.

use super::super::instance::INode;
use super::super::modifiers::{count_into, scramble_into};
use super::state::{NodeState, TextNow};
use super::TextBuf;
use crate::motion::Ease;

/// How far an animator running from `at` for `dur` seconds is at `t`, eased.
fn progress(at: f64, dur: f64, ease: Ease, t: f64) -> f64 {
    let p = if dur > 0.0 {
        ((t - at) / dur).clamp(0.0, 1.0)
    } else if t >= at {
        1.0
    } else {
        0.0
    };
    ease.apply(p)
}

/// How often a scramble's random characters change, a second.
const SCRAMBLE_RATE: f64 = 24.0;

pub(super) fn animate(n: &INode, t: f64, s: &mut NodeState, buf: Option<&mut TextBuf>) -> usize {
    let chars_of = |i: usize| n.text_chars.get(i).copied().unwrap_or(0) as usize;
    let shown = |s: &NodeState| match s.text {
        TextNow::Base => 0,
        TextNow::Text(i) | TextNow::Buffer(i) => usize::from(i),
    };
    let Some(a) = n.anim.as_deref() else {
        return chars_of(shown(s));
    };
    if s.text == TextNow::Base {
        let i = a.holds.partition_point(|(at, _)| *at <= t);
        if let Some(&(_, text)) = i.checked_sub(1).and_then(|i| a.holds.get(i)) {
            s.text = TextNow::Text(text);
        }
    }
    if let Some(r) = a.reveal {
        s.reveal = progress(r.at, r.dur, r.ease, t).clamp(0.0, 1.0);
    }
    let Some(buf) = buf else {
        return chars_of(shown(s));
    };
    let mut wrote = false;
    if let Some((timing, seed, chars)) = &a.scramble {
        let text = n.texts.get(shown(s)).map_or("", String::as_str);
        let p = progress(timing.at, timing.dur, timing.ease, t);
        let step = (t.max(timing.at) * SCRAMBLE_RATE)
            .floor()
            .clamp(0.0, f64::from(u32::MAX)) as u32;
        scramble_into(text, p, step, chars, *seed, buf);
        wrote = true;
    }
    if let Some(c) = &a.count {
        let p = progress(c.at, c.dur, c.ease, t);
        buf.clear();
        count_into(
            c.from + (c.to - c.from) * p,
            c.format,
            &c.prefix,
            &c.suffix,
            buf,
        );
        wrote = true;
    }
    if wrote {
        s.text = TextNow::Buffer(shown(s).min(usize::from(u16::MAX)) as u16);
        buf.chars().count()
    } else {
        chars_of(shown(s))
    }
}
