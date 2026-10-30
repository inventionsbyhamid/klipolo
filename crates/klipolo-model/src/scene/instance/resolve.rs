//! Bindings resolved for one layer's settings: each param's value (the layer's, else its default), held to its spec
//! (numbers within min–max, texts within maxLen, tables within maxRows), and one node's view of them, with the index
//! and row of the repeat around it (`$i`, `$row.label`, `$row.v0`…). Values that don't fit their field read as None,
//! so the field keeps its default. Built when a layer's settings change, never per frame.

use std::collections::BTreeMap;

use super::super::check::{binding, Binding};
use super::super::format::number_text;
use super::super::units::{parse_color, parse_percent, parse_px};
use super::super::{literal, Flag, TextCase, Val};
use crate::font::FontRef;
use crate::params::{ParamKind, ParamSpec, ParamValue, TableRow, MAX_ROW_LABEL, MAX_TABLE_ROWS};

/// A param's value as the scene reads it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PVal {
    Num(f64),
    Bool(bool),
    /// Texts, choices, colours and image ids.
    Text(String),
    Font(FontRef),
    Rows(Vec<TableRow>),
    Unset,
}

/// Every param's value, in the scene's order.
#[derive(Clone, Debug, Default)]
pub(crate) struct Values {
    keys: Vec<String>,
    vals: Vec<PVal>,
}

impl Values {
    pub fn resolve(specs: &[ParamSpec], given: &BTreeMap<String, ParamValue>) -> Self {
        let mut out = Self::default();
        for spec in specs {
            out.keys.push(spec.key.clone());
            out.vals.push(value(spec, given.get(&spec.key)));
        }
        out
    }

    pub fn get(&self, key: &str) -> Option<&PVal> {
        let i = self.keys.iter().position(|k| k == key)?;
        self.vals.get(i)
    }
}

/// One param's value: the layer's when it fits the spec (held to it), else the default.
fn value(spec: &ParamSpec, given: Option<&ParamValue>) -> PVal {
    let number = |lo: f64, hi: f64, default: f64| match given {
        Some(ParamValue::Number(n)) if n.is_finite() => PVal::Num(n.clamp(lo.min(hi), hi.max(lo))),
        _ => PVal::Num(default),
    };
    match &spec.kind {
        ParamKind::Float {
            min, max, default, ..
        } => number(*min, *max, *default),
        ParamKind::Int { min, max, default } => {
            match number(*min as f64, *max as f64, *default as f64) {
                PVal::Num(n) => PVal::Num(n.round()),
                other => other,
            }
        }
        ParamKind::Angle { default } => number(-360.0, 360.0, *default),
        ParamKind::Bool { default } => match given {
            Some(ParamValue::Bool(b)) => PVal::Bool(*b),
            _ => PVal::Bool(*default),
        },
        ParamKind::Color { default, .. } => match given {
            Some(ParamValue::Text(t)) if parse_color(t).is_some() => PVal::Text(t.clone()),
            _ => PVal::Text(default.clone()),
        },
        ParamKind::Choice { options, default } => match given {
            Some(ParamValue::Text(t)) if options.contains(t) => PVal::Text(t.clone()),
            _ => PVal::Text(default.clone()),
        },
        ParamKind::Text { default, max_len } => {
            let text = match given {
                Some(ParamValue::Text(t)) => t,
                _ => default,
            };
            PVal::Text(text.chars().take(*max_len as usize).collect())
        }
        ParamKind::Asset { .. } => match given {
            Some(ParamValue::Text(t)) => PVal::Text(t.clone()),
            _ => PVal::Unset,
        },
        ParamKind::Font { default } => match given {
            Some(ParamValue::Font(f)) => PVal::Font(f.clone()),
            _ => PVal::Font(default.clone()),
        },
        ParamKind::Table {
            columns,
            max_rows,
            default,
        } => {
            let rows = match given {
                Some(ParamValue::Rows(r)) => r,
                _ => default,
            };
            let most = (*max_rows).min(MAX_TABLE_ROWS) as usize;
            PVal::Rows(
                rows.iter()
                    .take(most)
                    .map(|r| TableRow {
                        label: r.label.chars().take(MAX_ROW_LABEL).collect(),
                        values: (0..columns.len())
                            .map(|c| {
                                r.values
                                    .get(c)
                                    .copied()
                                    .filter(|v| v.is_finite())
                                    .unwrap_or(0.0)
                            })
                            .collect(),
                    })
                    .collect(),
            )
        }
        ParamKind::Point { .. } => PVal::Unset,
    }
}

/// A node's view of the settings: the index of the repeat instance it is in, and its row.
#[derive(Clone, Copy)]
pub(crate) struct Ctx<'a> {
    pub vals: &'a Values,
    pub index: f64,
    pub row: Option<&'a TableRow>,
}

impl<'a> Ctx<'a> {
    /// A number: a literal, `"50%"`, or a number bound by `$key`, `$i` or `$row.vN`.
    pub fn num(&self, v: &Val) -> Option<f64> {
        let n = match v {
            Val::Num(n) => *n,
            Val::Str(s) => match binding(s) {
                Some(b) => self.bound_num(b)?,
                None => parse_percent(s)?,
            },
        };
        n.is_finite().then_some(n)
    }

    fn bound_num(&self, b: Binding) -> Option<f64> {
        match b {
            Binding::Param(key) => match self.vals.get(key)? {
                PVal::Num(n) => Some(*n),
                _ => None,
            },
            Binding::Index => Some(self.index),
            Binding::RowValue(n) => self.row?.values.get(n).copied(),
            Binding::RowLabel | Binding::BadRow => None,
        }
    }

    /// A length as a fraction of the box and pixels: a number (pixels when `px`), `"50%"`, `"22px"` or a binding.
    pub fn len(&self, v: &Val, px: bool) -> Option<[f64; 2]> {
        if let Val::Str(s) = v {
            if binding(s).is_none() {
                if let Some(p) = parse_px(s) {
                    return Some([0.0, p]);
                }
            }
        }
        let n = self.num(v)?;
        Some(if px { [0.0, n] } else { [n, 0.0] })
    }

    /// A colour, literal or bound.
    pub fn color(&self, s: &str) -> Option<[f32; 4]> {
        match binding(s) {
            Some(Binding::Param(key)) => match self.vals.get(key)? {
                PVal::Text(t) => parse_color(t),
                _ => None,
            },
            Some(_) => None,
            None => parse_color(s),
        }
    }

    pub fn flag(&self, f: &Flag) -> Option<bool> {
        match f {
            Flag::Bool(b) => Some(*b),
            Flag::Bind(s) => self.bound_flag(s),
        }
    }

    /// An on/off key value: 0 or 1, or a bound on/off setting.
    pub fn flag_val(&self, v: &Val) -> Option<bool> {
        match v {
            Val::Num(n) => Some(*n >= 0.5),
            Val::Str(s) => self.bound_flag(s),
        }
    }

    fn bound_flag(&self, s: &str) -> Option<bool> {
        match binding(s) {
            Some(Binding::Param(key)) => match self.vals.get(key)? {
                PVal::Bool(b) => Some(*b),
                _ => None,
            },
            _ => None,
        }
    }

    /// The text a string shows, `case` applied: a bound text or choice, the row's label, a bound number (in at most
    /// 24 characters), or the literal (`$$x` shows `$x`).
    pub fn text(&self, s: &str, case: Option<TextCase>) -> String {
        let shown = match binding(s) {
            Some(Binding::Param(key)) => match self.vals.get(key) {
                Some(PVal::Text(t)) => t.clone(),
                Some(PVal::Num(n)) => number_text(*n).to_string(),
                _ => String::new(),
            },
            Some(Binding::RowLabel) => self.row.map(|r| r.label.clone()).unwrap_or_default(),
            Some(b) => self
                .bound_num(b)
                .map(|n| number_text(n).to_string())
                .unwrap_or_default(),
            None => literal(s).to_owned(),
        };
        cased(shown, case)
    }

    /// A number shown as text, `case` applied.
    pub fn number_text(&self, n: f64, case: Option<TextCase>) -> String {
        cased(number_text(n).to_string(), case)
    }

    /// The rows a `"$table"` binding names.
    pub fn rows(&self, s: &str) -> Option<&'a [TableRow]> {
        match binding(s) {
            Some(Binding::Param(key)) => match self.vals.get(key)? {
                PVal::Rows(r) => Some(r.as_slice()),
                _ => None,
            },
            _ => None,
        }
    }

    /// The image id an `"$image"` binding names.
    pub fn asset(&self, s: &str) -> Option<&'a str> {
        match binding(s) {
            Some(Binding::Param(key)) => match self.vals.get(key)? {
                PVal::Text(t) => Some(t.as_str()),
                _ => None,
            },
            _ => None,
        }
    }

    /// The face a `"$font"` binding names.
    pub fn font(&self, s: &str) -> Option<&'a FontRef> {
        match binding(s) {
            Some(Binding::Param(key)) => match self.vals.get(key)? {
                PVal::Font(f) => Some(f),
                _ => None,
            },
            _ => None,
        }
    }
}

/// `s` as `case` writes it.
pub(crate) fn cased(s: String, case: Option<TextCase>) -> String {
    match case {
        None => s,
        Some(TextCase::Upper) => s.to_uppercase(),
        Some(TextCase::Lower) => s.to_lowercase(),
    }
}
