//! A text node: text or spans, its font (a `FontRef`'s JSON or the shorthand), outline and stagger (its animators
//! in `animators`), and how many characters and split units it can show at most, as its case writes them.

use super::super::problems::{at, join, Problems};
use super::super::{Node, SceneFacts, SceneFont, SceneLimits, Split};
use super::common::flag;
use super::nodes::NodeCost;
use super::paint::paint;
use super::ranges::limits;
use super::values::{num_field, range_val, text, text_len, val, Want};
use super::{Repeat, Scope};
use crate::font::font_ok;

pub(super) fn text_node(
    s: &Scope,
    out: &mut Problems,
    facts: &mut SceneFacts,
    n: &Node,
    path: &str,
    rep: Option<Repeat>,
) -> NodeCost {
    let p = &n.props;
    let mut chars = 0usize;
    match (&p.text, &p.spans) {
        (Some(t), None) => {
            text(s, out, t, Want::Text, &join(path, "text"), rep);
            chars = text_len(s, t, p.case);
        }
        (None, Some(spans)) => {
            let spans_path = join(path, "spans");
            if spans.len() > 32 {
                out.push(
                    spans_path.clone(),
                    "scene.value",
                    "A text has at most 32 spans.",
                );
            }
            for (i, span) in spans.iter().enumerate() {
                let span_path = at(&spans_path, i);
                text(
                    s,
                    out,
                    &span.text,
                    Want::Text,
                    &join(&span_path, "text"),
                    rep,
                );
                chars = chars.saturating_add(text_len(s, &span.text, p.case));
                if let Some(fill) = &span.fill {
                    paint(s, out, facts, fill, &join(&span_path, "fill"), rep);
                }
                num_field(
                    s,
                    out,
                    &span.size,
                    Want::Number,
                    &span_path,
                    "size",
                    rep,
                    limits("size"),
                );
                num_field(
                    s,
                    out,
                    &span.weight,
                    Want::Number,
                    &span_path,
                    "weight",
                    rep,
                    limits("weight"),
                );
                flag(s, out, &span.italic, &join(&span_path, "italic"), rep);
                if let Some(bg) = &span.background {
                    let bg_path = join(&span_path, "background");
                    text(
                        s,
                        out,
                        &bg.color,
                        Want::Color,
                        &join(&bg_path, "color"),
                        rep,
                    );
                    let sizes = [bg.padding, bg.radius];
                    if !sizes
                        .iter()
                        .flatten()
                        .all(|v| v.is_finite() && (0.0..=1000.0).contains(v))
                    {
                        out.push(
                            bg_path,
                            "scene.value",
                            "A span's padding and radius are 0 to 1000 pixels.",
                        );
                    }
                }
            }
        }
        (Some(_), Some(_)) => out.push(
            join(path, "spans"),
            "scene.value",
            "A text node has text or spans, not both.",
        ),
        (None, None) => out.push(
            join(path, "text"),
            "scene.required",
            "A text node needs text (or spans).",
        ),
    }
    if let Some(font) = &p.font {
        let font_path = join(path, "font");
        match font {
            SceneFont::Ref { font, weight, .. } => {
                if !font_ok(font) {
                    out.push(
                        font_path.clone(),
                        "scene.font",
                        "A font's family is 1 to 100 characters on one line; a Google font names its version (v31).",
                    );
                }
                if weight.is_some_and(|w| !(1..=1000).contains(&w)) {
                    out.push(
                        join(&font_path, "weight"),
                        "scene.font",
                        "A font's weight is 1 to 1000.",
                    );
                }
            }
            SceneFont::Name(name) => {
                text(s, out, name, Want::Font, &font_path, rep);
            }
        }
    }
    if let Some(outline) = &p.outline {
        let outline_path = join(path, "outline");
        let width = val(
            s,
            out,
            &outline.width,
            Want::Number,
            &join(&outline_path, "width"),
            rep,
        );
        range_val(
            s,
            out,
            &join(&outline_path, "width"),
            &outline.width,
            width,
            (0.0, 100.0),
        );
        text(
            s,
            out,
            &outline.color,
            Want::Color,
            &join(&outline_path, "color"),
            rep,
        );
    }
    if let Some(stagger) = &p.stagger {
        super::timing::stagger(s, out, stagger, &join(path, "stagger"));
    }
    // A text key shows its value in place of the text: the longest one counts.
    let (keyed_chars, keyed_words) = super::shown::keyed_text(s, n, p.case);
    let shown = chars.max(keyed_chars);
    chars = shown.saturating_add(super::animators::animators(s, out, n, path, rep));
    if chars > SceneLimits::MAX_TEXT_NODE {
        out.push(
            join(path, "text"),
            "scene.textTooLong",
            format!(
                "A text node shows at most {} characters; this one can show {chars} (its settings' longest).",
                SceneLimits::MAX_TEXT_NODE
            ),
        );
    }
    let units = match p.split {
        None => 0,
        Some(Split::Chars) => chars,
        Some(Split::Words) => super::shown::shown_words(s, p, chars).max(keyed_words),
        // Lines come from the layout, which wraps titles with `Wrap::WordOrGlyph` (the engine's text layer): a word
        // wider than the box breaks inside it, so a line can hold one character. Whatever it wraps, a text makes at
        // most as many lines as it has characters.
        Some(Split::Lines) => chars,
    };
    NodeCost {
        particles: 0,
        chars,
        shown,
        units,
    }
}
