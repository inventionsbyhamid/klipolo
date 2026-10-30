//! Bindings: `"$key"` names a declared param whose kind fits the field (text or choice for text, colour for paints,
//! number for numbers, font, image, table, on/off); inside a `repeat`, `$i` and the row's `$row.label` and
//! `$row.v0`…`$row.v3`.

use super::super::problems::{quote, Problems};
use super::super::{SceneLimits, Val};
use super::values::Want;
use super::{Repeat, Scope};
use crate::params::ParamKind;
use crate::project::AssetKind;

/// A `"$…"` string: a param, or inside a repeat the index and the row's label and numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::scene) enum Binding<'s> {
    Param(&'s str),
    Index,
    RowLabel,
    RowValue(usize),
    BadRow,
}

/// The binding a string is, if it is one: `$` and a name. `$$…` and `$5` are text.
pub(in crate::scene) fn binding(s: &str) -> Option<Binding<'_>> {
    let rest = s.strip_prefix('$')?;
    let first = rest.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if rest == "i" {
        return Some(Binding::Index);
    }
    if rest == "row" || rest.starts_with("row.") {
        return Some(match rest.strip_prefix("row.") {
            Some("label") => Binding::RowLabel,
            Some("v0") => Binding::RowValue(0),
            Some("v1") => Binding::RowValue(1),
            Some("v2") => Binding::RowValue(2),
            Some("v3") => Binding::RowValue(3),
            _ => Binding::BadRow,
        });
    }
    rest.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        .then_some(Binding::Param(rest))
}

/// The largest number a bound setting gives, for costs.
fn param_max(kind: &ParamKind) -> Option<f64> {
    match kind {
        ParamKind::Float { max, .. } => Some(*max),
        ParamKind::Int { max, .. } => Some(*max as f64),
        ParamKind::Angle { .. } => Some(360.0),
        _ => None,
    }
}

fn fits(kind: &ParamKind, want: Want) -> bool {
    match want {
        Want::Number | Want::Position => {
            matches!(
                kind,
                ParamKind::Float { .. } | ParamKind::Int { .. } | ParamKind::Angle { .. }
            )
        }
        Want::Color => matches!(kind, ParamKind::Color { .. }),
        Want::Text => matches!(kind, ParamKind::Text { .. } | ParamKind::Choice { .. }),
        Want::Font => matches!(kind, ParamKind::Font { .. }),
        Want::Asset => matches!(
            kind,
            ParamKind::Asset {
                kind: AssetKind::Image
            }
        ),
        Want::Table => matches!(kind, ParamKind::Table { .. }),
        Want::Flag => matches!(kind, ParamKind::Bool { .. }),
    }
}

fn kind_noun(kind: &ParamKind) -> &'static str {
    match kind {
        ParamKind::Float { .. } | ParamKind::Int { .. } | ParamKind::Angle { .. } => {
            "a number setting"
        }
        ParamKind::Color { .. } => "a colour setting",
        ParamKind::Text { .. } => "a text setting",
        ParamKind::Choice { .. } => "a choice setting",
        ParamKind::Font { .. } => "a font setting",
        ParamKind::Asset { .. } => "an asset setting",
        ParamKind::Table { .. } => "a table setting",
        ParamKind::Bool { .. } => "an on/off setting",
        ParamKind::Point { .. } => "a point setting",
    }
}

/// Checks a binding where `want` is wanted; the largest number it gives, when it gives one.
pub(super) fn bind(
    s: &Scope,
    out: &mut Problems,
    b: Binding,
    want: Want,
    path: &str,
    repeat: Option<Repeat>,
) -> Option<f64> {
    let numbers = matches!(want, Want::Number | Want::Position | Want::Text);
    match b {
        Binding::Param(key) => match s.params.get(key) {
            None if out.full() => {
                out.skip();
                None
            }
            None => {
                // At most `MAX_PARAMS` keys: a scene with more stops at its settings (`check_scene`).
                let declared: Vec<&str> = s
                    .params
                    .keys()
                    .copied()
                    .take(SceneLimits::MAX_PARAMS)
                    .collect();
                let list = if declared.is_empty() {
                    "it declares none".to_owned()
                } else {
                    format!("they are {}", declared.join(", "))
                };
                out.push(
                    path,
                    "scene.bindingUnknown",
                    format!("`${}` names no setting of this scene; {list}.", quote(key)),
                );
                None
            }
            Some(spec) if fits(&spec.kind, want) => param_max(&spec.kind),
            Some(spec) => {
                out.push(
                    path,
                    "scene.bindingKind",
                    format!(
                        "`${key}` is {}, but this field takes {}.",
                        kind_noun(&spec.kind),
                        want.noun()
                    ),
                );
                None
            }
        },
        Binding::Index if repeat.is_some() && numbers => Some(1000.0),
        Binding::RowLabel if matches!(repeat, Some(Repeat::Rows { .. })) && want == Want::Text => {
            None
        }
        Binding::RowValue(n)
            if numbers && matches!(repeat, Some(Repeat::Rows { columns }) if n < columns) =>
        {
            None
        }
        Binding::Index => {
            unknown_in_repeat(
                out,
                path,
                "`$i` is a repeat's instance index: a number, inside a repeat.",
            );
            None
        }
        Binding::RowLabel | Binding::RowValue(_) | Binding::BadRow => {
            unknown_in_repeat(
                out,
                path,
                "`$row.label` (text) and `$row.v0`…`$row.v3` (one per table column) bind a row inside a repeat \
                 over a table setting (\"rows\": \"$key\").",
            );
            None
        }
    }
}

fn unknown_in_repeat(out: &mut Problems, path: &str, message: &str) {
    out.push(path, "scene.bindingUnknown", message);
}

/// A count (a repeat's `times`, an emitter's `burst` or `rate`) at its largest: a literal number, or a `$key`
/// whole-number or number setting at its maximum. A count that could come from anything without a maximum Klipolo can
/// check (`$i`, a table row, an angle setting, a percentage) is refused, so no cost below it can be emptied.
/// `repeat_count` and the field checks both read it, so they agree.
pub(super) fn count(s: &Scope, v: &Val) -> Result<f64, (&'static str, String)> {
    let text = match v {
        Val::Num(n) if n.is_finite() => return Ok(*n),
        Val::Num(_) => return Err(("scene.value", "Numbers are finite.".to_owned())),
        Val::Str(text) => text,
    };
    match binding(text) {
        Some(Binding::Param(key)) => match s.params.get(key).map(|p| &p.kind) {
            Some(ParamKind::Int { max, .. }) => Ok(*max as f64),
            Some(ParamKind::Float { max, .. }) if max.is_finite() => Ok(*max),
            Some(kind) => Err((
                "scene.bindingKind",
                format!(
                    "`${key}` is {}; a count takes a whole-number or number setting, counted at its maximum.",
                    kind_noun(kind)
                ),
            )),
            None => Err((
                "scene.bindingUnknown",
                format!("`${}` names no setting of this scene.", quote(key)),
            )),
        },
        Some(_) => Err((
            "scene.bindingKind",
            "A count can't come from `$i` or a table row: Klipolo can't check how large it gets. Write a number or \
             \"$key\" (a number setting)."
                .to_owned(),
        )),
        None => Err((
            "scene.value",
            format!(
                "\"{}\" isn't a count: write a number or \"$key\" (a number setting).",
                quote(text)
            ),
        )),
    }
}

/// The range of the number setting a string binds, if it binds one.
pub(super) fn param_range(s: &Scope, text: &str) -> Option<(f64, f64)> {
    let Some(Binding::Param(key)) = binding(text) else {
        return None;
    };
    match s.params.get(key).map(|p| &p.kind) {
        Some(ParamKind::Int { min, max, .. }) => Some((*min as f64, *max as f64)),
        Some(ParamKind::Float { min, max, .. }) => Some((*min, *max)),
        Some(ParamKind::Angle { .. }) => Some((-360.0, 360.0)),
        _ => None,
    }
}
