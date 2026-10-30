//! Checking one value: what the field wants (a number, a position, a colour, a text, a font, an image, a table or an
//! on/off), literals in range, and bindings that name a declared param of the right kind (or, inside a `repeat`,
//! `$i` and the row's fields).

use super::super::keys::ValueKind;
use super::super::problems::{join, quote, Problems};
use super::super::units::{parse_color, parse_percent, parse_px, MAX_PX};
use super::super::{TextCase, Val};
pub(super) use super::bind::{bind, binding, count, param_range, Binding};
use super::{Repeat, Scope};
use crate::font::family_ok;
use crate::params::{ParamKind, MAX_ROW_LABEL};

/// What a field takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Want {
    Number,
    /// A number, or a length: `"50%"`, `"22px"`.
    Position,
    Color,
    Text,
    Font,
    Asset,
    Table,
    Flag,
}

impl Want {
    pub fn of(kind: ValueKind) -> Want {
        match kind {
            ValueKind::Position => Want::Position,
            ValueKind::Number => Want::Number,
            ValueKind::Color => Want::Color,
            ValueKind::Flag => Want::Flag,
            ValueKind::Text | ValueKind::Clip | ValueKind::Origin => Want::Text,
        }
    }

    pub(super) fn noun(self) -> &'static str {
        match self {
            Want::Number | Want::Position => "a number setting",
            Want::Color => "a colour setting",
            Want::Text => "a text or choice setting",
            Want::Font => "a font setting",
            Want::Asset => "an image setting",
            Want::Table => "a table setting",
            Want::Flag => "an on/off setting",
        }
    }
}

/// A string value: a binding, or a literal of what `want` takes. The largest number it gives, when it gives one
/// (positions in pixels give none).
pub(super) fn text(
    s: &Scope,
    out: &mut Problems,
    v: &str,
    want: Want,
    path: &str,
    repeat: Option<Repeat>,
) -> Option<f64> {
    if let Some(b) = binding(v) {
        return bind(s, out, b, want, path, repeat);
    }
    let shown = quote(v);
    match want {
        Want::Number => match parse_percent(v) {
            Some(f) => return Some(f),
            None => out.push(
                path,
                "scene.value",
                format!("\"{shown}\" isn't a number: write a number, \"50%\" or \"$key\"."),
            ),
        },
        Want::Position => {
            if let Some(f) = parse_percent(v) {
                return Some(f);
            }
            match parse_px(v) {
                Some(px) if px.abs() <= MAX_PX => {}
                Some(_) => out.push(
                    path,
                    "scene.value",
                    format!("\"{shown}\" is out of range: pixel positions go from -{MAX_PX} to {MAX_PX}."),
                ),
                None => out.push(
                    path,
                    "scene.value",
                    format!("\"{shown}\" isn't a position: write a fraction of the box, \"50%\", \"22px\" or \"$key\"."),
                ),
            }
        }
        Want::Color => {
            if parse_color(v).is_none() {
                out.push(
                    path,
                    "scene.color",
                    format!("\"{shown}\" isn't a colour: write #RGB, #RRGGBB, #RRGGBBAA, rgb(…), rgba(…) or \"$key\"."),
                );
            }
        }
        Want::Text => {}
        Want::Font => {
            if !family_ok(super::super::literal(v)) {
                out.push(path, "scene.font", "A font family is 1 to 100 characters on one line.");
            }
        }
        Want::Asset => out.push(
            path,
            "scene.value",
            "An image comes from an image setting: write \"$key\" and declare {\"type\": \"asset\", \"kind\": \
             \"image\"}.",
        ),
        Want::Table => out.push(path, "scene.value", "This comes from a table setting: write \"$key\"."),
        Want::Flag => out.push(path, "scene.value", "Write true, false or \"$key\" (an on/off setting)."),
    }
    None
}

/// A number-or-string value, checked as [`text`] for strings; numbers finite, and 0 or 1 for an on/off.
pub(super) fn val(
    s: &Scope,
    out: &mut Problems,
    v: &Val,
    want: Want,
    path: &str,
    repeat: Option<Repeat>,
) -> Option<f64> {
    match v {
        Val::Str(t) => text(s, out, t, want, path, repeat),
        Val::Num(n) if !n.is_finite() => {
            out.push(path, "scene.value", "Numbers are finite.");
            None
        }
        Val::Num(n) => match want {
            Want::Number | Want::Position | Want::Text => Some(*n),
            Want::Flag if *n == 0.0 || *n == 1.0 => Some(*n),
            Want::Flag => {
                out.push(
                    path,
                    "scene.value",
                    "An on/off value in keys and tweens is 0 or 1.",
                );
                None
            }
            Want::Color => {
                out.push(
                    path,
                    "scene.color",
                    "A colour is a string such as \"#FFB000\".",
                );
                None
            }
            Want::Font | Want::Asset | Want::Table => {
                out.push(
                    path,
                    "scene.value",
                    format!("This takes {}, not a number.", want.noun()),
                );
                None
            }
        },
    }
}

/// `value`, when known, from `lo` to `hi`.
pub(super) fn range(out: &mut Problems, path: &str, value: Option<f64>, lo: f64, hi: f64) {
    if let Some(v) = value {
        if !(lo..=hi).contains(&v) {
            out.push(
                path,
                "scene.value",
                format!("{v} is out of range: this goes from {lo} to {hi}."),
            );
        }
    }
}

/// An optional number field, checked and in range.
#[allow(clippy::too_many_arguments)]
pub(super) fn num_field(
    s: &Scope,
    out: &mut Problems,
    v: &Option<Val>,
    want: Want,
    path: &str,
    key: &str,
    repeat: Option<Repeat>,
    (lo, hi): (f64, f64),
) -> Option<f64> {
    let v = v.as_ref()?;
    let path = join(path, key);
    let max = val(s, out, v, want, &path, repeat);
    range_val(s, out, &path, v, max, (lo, hi));
    max
}

/// A number value from `lo` to `hi`: a literal, a percentage or `$i` at its value, and a bound number setting at both
/// ends of its own range, so neither its minimum nor its maximum falls outside.
pub(super) fn range_val(
    s: &Scope,
    out: &mut Problems,
    path: &str,
    v: &Val,
    most: Option<f64>,
    (lo, hi): (f64, f64),
) {
    if let Val::Str(t) = v {
        if let Some((from, to)) = param_range(s, t) {
            if from < lo || to > hi {
                out.push(
                    path,
                    "scene.value",
                    format!(
                        "`{}` goes from {from} to {to}; this takes {lo} to {hi}.",
                        quote(t)
                    ),
                );
            }
            return;
        }
    }
    range(out, path, most, lo, hi);
}

/// Seconds inside the scene.
pub(super) fn seconds(s: &Scope, out: &mut Problems, v: Option<f64>, path: &str) {
    if let Some(t) = v {
        if !(t.is_finite() && (0.0..=s.scene.duration).contains(&t)) {
            out.push(
                path,
                "scene.value",
                format!("{t} s is outside the scene (0 to {} s).", s.scene.duration),
            );
        }
    }
}

/// The most characters a bound number shows, as a counter's are counted (the evaluator formats bound numbers within
pub(super) const NUMBER_CHARS: usize = 24;

/// How many characters a text value can show: its own as `case` writes them, or a bound setting's longest (three
/// times over upper-cased and twice lower-cased, the most a character grows: ß → SS, ﬃ → FFI); a bound number
/// ([`NUMBER_CHARS`]) or other value at most 24.
pub(super) fn text_len(s: &Scope, v: &str, case: Option<TextCase>) -> usize {
    let grown = |n: usize| match case {
        None => n,
        Some(TextCase::Upper) => n.saturating_mul(3),
        Some(TextCase::Lower) => n.saturating_mul(2),
    };
    match binding(v) {
        Some(Binding::Param(key)) => match s.params.get(key).map(|p| &p.kind) {
            Some(ParamKind::Text { max_len, .. }) => grown(*max_len as usize),
            Some(ParamKind::Choice { options, .. }) => {
                grown(options.iter().map(|o| o.chars().count()).max().unwrap_or(0))
            }
            _ => NUMBER_CHARS,
        },
        Some(Binding::RowLabel) => grown(MAX_ROW_LABEL),
        Some(_) => NUMBER_CHARS,
        None => match case {
            None => v.chars().count(),
            Some(TextCase::Upper) => v.chars().flat_map(char::to_uppercase).count(),
            Some(TextCase::Lower) => v.chars().flat_map(char::to_lowercase).count(),
        },
    }
}
