//! A scene text's font: `font` as written (a `FontRef`), the shorthand through
//! `resolve_family`, or a `"$font"` setting; and the weight and italic it is drawn at.

use klipolo_fonts::index::FontIndex;
use klipolo_model::font::{FontRef, FontSource};
use klipolo_model::scene::instance::{Field, SceneInstance};
use klipolo_model::scene::{Flag, Node, SceneFont, Val, literal};

use super::text::DEFAULT_FAMILY;

/// How a text node's bound fields read: its instance's settings, or none (`NoBindings`).
pub trait Bindings {
    fn number(&self, field: Field, v: &Val) -> Option<f64>;
    fn flag(&self, f: &Flag) -> Option<bool>;
    fn font(&self, s: &str) -> Option<FontRef>;
}

/// No settings: literals only.
pub struct NoBindings;

impl Bindings for NoBindings {
    fn number(&self, field: Field, v: &Val) -> Option<f64> {
        let (lo, hi) = field.range();
        match v {
            Val::Num(n) if n.is_finite() => Some(n.clamp(lo, hi)),
            _ => None,
        }
    }
    fn flag(&self, f: &Flag) -> Option<bool> {
        match f {
            Flag::Bool(b) => Some(*b),
            Flag::Bind(_) => None,
        }
    }
    fn font(&self, _: &str) -> Option<FontRef> {
        None
    }
}

/// Instance node `node`'s settings, repeat index and row.
pub struct NodeBindings<'a> {
    pub inst: &'a SceneInstance,
    pub node: usize,
}

impl Bindings for NodeBindings<'_> {
    fn number(&self, field: Field, v: &Val) -> Option<f64> {
        self.inst.number_of(self.node, field, v)
    }
    fn flag(&self, f: &Flag) -> Option<bool> {
        self.inst.flag_of(self.node, f)
    }
    fn font(&self, s: &str) -> Option<FontRef> {
        self.inst.font_of(self.node, s).cloned()
    }
}

/// A `"$key"` string (a binding), not a literal `$$…` or `$5`.
fn is_binding(s: &str) -> bool {
    s.strip_prefix('$')
        .and_then(|r| r.chars().next())
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
}

/// The font `f` of text node `node` names, and the weight and italic it is drawn at: the `FontRef`
/// as written; the shorthand through `index.resolve_family` (bundled, then a downloaded Google family, then installed),
/// a name it can't find a missing installed family (the stand-in and `frame.fontMissing`); a `"$font"` binding the
/// setting's `FontRef`. Weight and italic come from `font` when it sets them, else from the node's own (else 400,
/// upright). `family_for` then picks the exact face.
pub fn font_ref(
    f: &SceneFont,
    node: &Node,
    index: &FontIndex,
    params: &impl Bindings,
) -> (FontRef, u16, bool) {
    let (own_weight, own_italic) = own_look(node, params);
    match f {
        SceneFont::Ref {
            font,
            weight,
            italic,
        } => (
            font.clone(),
            weight.unwrap_or(own_weight),
            italic.unwrap_or(own_italic),
        ),
        SceneFont::Name(name) => {
            let font = if is_binding(name) {
                params
                    .font(name)
                    .unwrap_or_else(|| FontRef::bundled(DEFAULT_FAMILY))
            } else {
                let name = literal(name);
                index.resolve_family(name).unwrap_or_else(|| FontRef {
                    family: name.trim().to_string(),
                    source: FontSource::Installed,
                })
            };
            (font, own_weight, own_italic)
        }
    }
}

/// The node's own weight (400 unless set) and italic.
pub(super) fn own_look(node: &Node, params: &impl Bindings) -> (u16, bool) {
    let p = &node.props;
    let weight = p
        .weight
        .as_ref()
        .and_then(|v| params.number(Field::Weight, v))
        .map_or(400, |w| w.round().clamp(1.0, 1000.0) as u16);
    let italic = p
        .italic
        .as_ref()
        .and_then(|f| params.flag(f))
        .unwrap_or(false);
    (weight, italic)
}
