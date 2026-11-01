//! What a text node can show in place of its text, counted as upper bounds: text keys and tween `text` values show
//! instead of the text (the longest counts), and split words count the words of every text the node can show.

use super::super::problems::{at, join, Problems};
use super::super::{Node, NodeType, Props, SceneLimits, Split, TextCase, Val};
use super::nodes::NodeCost;
use super::timeline::Placed;
use super::values::{binding, text_len, NUMBER_CHARS};
use super::Scope;

/// The words a text value can show: its own, or half a bound value's characters (a word and a space each).
fn words(s: &Scope, v: &str, case: Option<TextCase>) -> usize {
    if binding(v).is_some() {
        text_len(s, v, case).div_ceil(2)
    } else {
        v.split_whitespace().count()
    }
}

/// The longest text key, in characters as `case` writes them, and the most words one shows.
pub(super) fn keyed_text(s: &Scope, n: &Node, case: Option<TextCase>) -> (usize, usize) {
    n.animate
        .get("text")
        .into_iter()
        .flatten()
        .fold((0, 0), |(c, w), key| match &key.v {
            Val::Str(t) => (c.max(text_len(s, t, case)), w.max(words(s, t, case))),
            Val::Num(_) => (c.max(NUMBER_CHARS), w.max(1)),
        })
}

/// The most words a split-by-words text shows at once: the most of its text (or its spans), each hold, and the
/// counter's prefix, number and suffix; half its characters when a scramble may draw spaces.
pub(super) fn shown_words(s: &Scope, p: &Props, chars: usize) -> usize {
    let words = |t: &str| words(s, t, p.case);
    let mut n = match (&p.text, &p.spans) {
        (Some(t), _) => words(t),
        (None, Some(spans)) => spans
            .iter()
            .map(|span| words(&span.text))
            .fold(0, usize::saturating_add),
        _ => 0,
    };
    for hold in p.holds.iter().flatten() {
        n = n.max(words(&hold.text));
    }
    if let Some(count) = &p.count {
        let affix = |a: &Option<String>| a.as_deref().map_or(0, |t| t.split_whitespace().count());
        n = n.max(affix(&count.prefix) + affix(&count.suffix) + 1);
    }
    if p.scramble.is_some() {
        n = n.max(chars.div_ceil(2));
    }
    n
}

/// A tween's `text` (or `fromTo` start text) shows in place of each target's text: the longest counts toward its
/// characters and units, and a target it takes past the cap is refused at the tween.
pub(super) fn tween_texts(
    s: &Scope,
    out: &mut Problems,
    times: &[Option<Placed>],
    costs: &mut [NodeCost],
) {
    for (ti, (t, time)) in s.scene.timeline.iter().zip(times).enumerate() {
        let Some(placed) = time else {
            continue;
        };
        let tween_path = at("/timeline", ti);
        let start = t.start.as_ref().and_then(|st| st.get("text"));
        for (value, path) in [
            (t.vars.get("text"), join(&tween_path, "text")),
            (start, join(&join(&tween_path, "start"), "text")),
        ] {
            let Some(value) = value else {
                continue;
            };
            // Measured once per case (none, upper, lower), not once per target.
            let mut measured: [Option<(usize, usize)>; 3] = [None; 3];
            for &i in &placed.targets {
                let (Some(f), Some(cost)) = (s.flat.get(i), costs.get_mut(i)) else {
                    continue;
                };
                if f.node.kind != NodeType::Text {
                    continue;
                }
                let case = f.node.props.case;
                let slot = match case {
                    None => 0,
                    Some(TextCase::Upper) => 1,
                    Some(TextCase::Lower) => 2,
                };
                let measure = || match value {
                    Val::Str(text) => (text_len(s, text, case), words(s, text, case)),
                    Val::Num(_) => (NUMBER_CHARS, 1),
                };
                let (len, shown_words) = match measured.get_mut(slot) {
                    Some(cell) => *cell.get_or_insert_with(measure),
                    None => measure(),
                };
                if len > cost.shown {
                    cost.chars = cost.chars.saturating_add(len - cost.shown);
                    cost.shown = len;
                    if cost.chars > SceneLimits::MAX_TEXT_NODE && !out.has_path(&path) {
                        out.push(
                            path.clone(),
                            "scene.textTooLong",
                            format!(
                                "This text makes the node at {} show {} characters; a text node shows at most {}.",
                                f.path,
                                cost.chars,
                                SceneLimits::MAX_TEXT_NODE
                            ),
                        );
                    }
                }
                cost.units = match f.node.props.split {
                    None => 0,
                    Some(Split::Words) => cost.units.max(shown_words),
                    Some(_) => cost.chars,
                };
            }
        }
    }
}
