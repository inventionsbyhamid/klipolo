//! A text node's own text and its animators: the counter, the typewriter, scrambles and holds; and the units it lays
//! out before the renderer reports them.

use arrayvec::ArrayVec;

use super::super::format::{NumberFormat, MAX_COUNT};
use super::super::{Node, NodeType, SceneLimits, Split, Val, DEFAULT_TWEEN_DUR};
use super::statics::ease;
use super::Ctx;
use crate::motion::Ease;

/// A text node's own text: its text, or its spans' texts one after another, settings resolved and `case` applied.
pub(crate) fn own_text(node: &Node, ctx: &Ctx) -> String {
    let p = &node.props;
    match (&p.text, &p.spans) {
        (Some(t), _) => ctx.text(t, p.case),
        (None, Some(spans)) => spans.iter().map(|s| ctx.text(&s.text, p.case)).collect(),
        _ => String::new(),
    }
}

/// The counter: from, to, when and how, and how it writes its number.
#[derive(Clone, Debug)]
pub(crate) struct CountAnim {
    pub from: f64,
    pub to: f64,
    pub at: f64,
    pub dur: f64,
    pub ease: Ease,
    pub format: NumberFormat,
    pub prefix: String,
    pub suffix: String,
}

/// A typewriter or scramble's timing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timed {
    pub at: f64,
    pub dur: f64,
    pub ease: Ease,
}

/// What a text node's animators do, with its holds as (time, text index).
#[derive(Clone, Debug, Default)]
pub(crate) struct TextAnim {
    pub count: Option<CountAnim>,
    pub reveal: Option<Timed>,
    pub scramble: Option<(Timed, u32, ArrayVec<char, 64>)>,
    pub holds: Vec<(f64, u16)>,
}

const SCRAMBLE_CHARS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789#%&*+-=?@";

/// A text node's animators; its hold texts are added to `texts`. None without any.
pub(crate) fn text_anim(
    node: &Node,
    ctx: &Ctx,
    seed: u32,
    texts: &mut Vec<String>,
) -> Option<TextAnim> {
    let p = &node.props;
    if node.kind != NodeType::Text {
        return None;
    }
    let timed = |at: Option<f64>, dur: Option<f64>, e: Option<&str>, default: Ease| Timed {
        at: at.filter(|v| v.is_finite()).unwrap_or(0.0),
        dur: dur
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(DEFAULT_TWEEN_DUR),
        ease: ease(e, default),
    };
    let mut anim = TextAnim::default();
    if let Some(c) = &p.count {
        let held = |v: &Val| ctx.num(v).unwrap_or(0.0).clamp(-MAX_COUNT, MAX_COUNT);
        let t = timed(c.at, c.dur, c.ease.as_deref(), Ease::Power1Out);
        let affix =
            |a: &Option<String>| super::resolve::cased(a.clone().unwrap_or_default(), p.case);
        anim.count = Some(CountAnim {
            from: held(&c.from),
            to: held(&c.to),
            at: t.at,
            dur: t.dur,
            ease: t.ease,
            format: NumberFormat {
                group: c.separator == Some(true),
                decimals: c.decimals.unwrap_or(0).min(6),
                min_int: 1,
            },
            prefix: affix(&c.prefix),
            suffix: affix(&c.suffix),
        });
    }
    if let Some(r) = &p.reveal {
        anim.reveal = Some(timed(r.at, r.dur, r.ease.as_deref(), Ease::Linear));
    }
    if let Some(sc) = &p.scramble {
        let chars: ArrayVec<char, 64> = sc
            .chars
            .as_deref()
            .unwrap_or(SCRAMBLE_CHARS)
            .chars()
            .filter(|c| !c.is_control())
            .take(64)
            .collect();
        anim.scramble = Some((
            timed(sc.at, sc.dur, None, Ease::Linear),
            sc.seed.unwrap_or(seed),
            chars,
        ));
    }
    for h in p.holds.iter().flatten().take(SceneLimits::MAX_KEYS) {
        if h.t.is_finite() && texts.len() < usize::from(u16::MAX) {
            texts.push(ctx.text(&h.text, p.case));
            anim.holds.push((h.t, (texts.len() - 1) as u16));
        }
    }
    anim.holds.sort_by(|a, b| a.0.total_cmp(&b.0));
    let any = anim.count.is_some()
        || anim.reveal.is_some()
        || anim.scramble.is_some()
        || !anim.holds.is_empty();
    any.then_some(anim)
}

/// The units a split text lays out at most, estimated from its texts before the renderer reports them: characters
/// without spaces, whitespace runs' words (a counter's number is one word), or lines.
pub(crate) fn units_estimate(node: &Node, texts: &[String], anim: Option<&TextAnim>) -> usize {
    let Some(split) = node.props.split else {
        return 0;
    };
    let one = |t: &str| match split {
        Split::Chars => t.chars().filter(|c| !c.is_whitespace()).count(),
        Split::Words => t.split_whitespace().count(),
        Split::Lines => t.lines().filter(|l| !l.trim().is_empty()).count(),
    };
    let mut most = texts.iter().map(|t| one(t)).max().unwrap_or(0);
    if let Some(c) = anim.and_then(|a| a.count.as_ref()) {
        let words = c.prefix.split_whitespace().count() + 1 + c.suffix.split_whitespace().count();
        let chars = c.prefix.chars().count() + 24 + c.suffix.chars().count();
        most = most.max(if split == Split::Words { words } else { chars });
    }
    most
}
