//! What the renderer reads of a text instance node to lay it out: every text it can show, its spans'
//! texts, its on/off fields, and what its counter and scramble draw from. Read when the instance's texts are laid out
//! (a build, a resize, a font change), never per frame.

use super::super::format::NumberFormat;
use super::super::Flag;
use super::SceneInstance;

/// A counter's glyph needs: the values it runs between, how it writes them, and its prefix and suffix (case applied).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CounterText<'a> {
    pub from: f64,
    pub to: f64,
    pub format: NumberFormat,
    pub prefix: &'a str,
    pub suffix: &'a str,
}

impl SceneInstance {
    /// Every text instance node `node` can show, case applied: its own first (index 0, what `TextNow::Base` shows),
    /// then its holds, text keys and tween texts (`TextNow::Text(i)`). Empty for a node that isn't one.
    pub fn texts(&self, node: usize) -> &[String] {
        self.nodes.get(node).map_or(&[], |n| n.texts.as_slice())
    }

    /// Its spans' texts in order, case applied: their concatenation is its own text. Empty without spans.
    pub fn span_texts(&self, node: usize) -> Vec<String> {
        let Some(cn) = self
            .nodes
            .get(node)
            .and_then(|n| self.compiled().nodes.get(n.cnode))
        else {
            return Vec::new();
        };
        let p = &cn.node.props;
        if p.text.is_some() {
            return Vec::new();
        }
        let ctx = self.ctx(node);
        p.spans
            .iter()
            .flatten()
            .map(|s| ctx.text(&s.text, p.case))
            .collect()
    }

    /// An on/off field of `node` (a span's `italic`), literal or bound.
    pub fn flag_of(&self, node: usize, f: &Flag) -> Option<bool> {
        self.ctx(node).flag(f)
    }

    /// `node`'s counter, when it has one: what a frame's number can be and how it is written.
    pub fn counter(&self, node: usize) -> Option<CounterText<'_>> {
        let c = self.nodes.get(node)?.anim.as_deref()?.count.as_ref()?;
        Some(CounterText {
            from: c.from,
            to: c.to,
            format: c.format,
            prefix: &c.prefix,
            suffix: &c.suffix,
        })
    }

    /// The characters `node`'s scramble draws its random letters from, when it has one.
    pub fn scramble_chars(&self, node: usize) -> Option<&[char]> {
        let (_, _, chars) = self.nodes.get(node)?.anim.as_deref()?.scramble.as_ref()?;
        Some(chars.as_slice())
    }
}
