//! What the renderer draws for a setting: the one reading of a stored value that
//! `pack_params` packs, the envelope's clamp moves (`make::envelope::clamp`) and the verified store checks (`usable`), so
//! a value the engine can't draw as it is resolves the same way in all three. A number is clamped to its definition's
//! bounds (an `Int` rounded first); a value of the wrong type, an option the definition doesn't have, or a number that
//! isn't finite is the default (a choice whose default isn't an option either is its first option).

use klipolo_model::params::{ParamKind, ParamSpec, ParamValue, PointN};

/// A setting as drawn. Nothing here allocates: `pack_params` reads it every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drawn<'a> {
    /// A float, int or angle (degrees).
    Number(f64),
    Bool(bool),
    /// A choice's option index.
    Choice(u32),
    Point(PointN),
    /// Straight RGBA, 0–1.
    Color([f32; 4]),
    /// A kind shaders don't take (text, media, a font, a table): the value as stored.
    Other(Option<&'a ParamValue>),
}

/// How `spec`'s setting draws with `value` stored (None: the layer leaves it out).
pub fn drawn<'a>(spec: &ParamSpec, value: Option<&'a ParamValue>) -> Drawn<'a> {
    match &spec.kind {
        ParamKind::Float {
            min, max, default, ..
        } => Drawn::Number(within(number(value, *default), *min, *max)),
        ParamKind::Angle { default } => Drawn::Number(number(value, *default)),
        ParamKind::Int { min, max, default } => Drawn::Number(within(
            number(value, *default as f64).round(),
            *min as f64,
            *max as f64,
        )),
        ParamKind::Bool { default } => Drawn::Bool(match value {
            Some(ParamValue::Bool(b)) => *b,
            _ => *default,
        }),
        ParamKind::Choice { options, default } => {
            let pick = |s: &str| options.iter().position(|o| o == s);
            let index = text(value)
                .and_then(pick)
                .or_else(|| pick(default))
                .unwrap_or(0);
            Drawn::Choice(index as u32)
        }
        ParamKind::Point { default } => Drawn::Point(match value {
            Some(ParamValue::Point(p)) if p.x.is_finite() && p.y.is_finite() => *p,
            _ => *default,
        }),
        ParamKind::Color { default, .. } => Drawn::Color(
            text(value)
                .and_then(rgba_unit)
                .or_else(|| rgba_unit(default))
                .unwrap_or([0.0, 0.0, 0.0, 1.0]),
        ),
        ParamKind::Text { .. }
        | ParamKind::Asset { .. }
        | ParamKind::Font { .. }
        | ParamKind::Table { .. } => Drawn::Other(value),
    }
}

/// [`drawn`] as a stored value: what a layer holding `value` draws with. None for a kind shaders don't take that the
/// layer leaves out, or a choice with no options.
pub fn effective(spec: &ParamSpec, value: Option<&ParamValue>) -> Option<ParamValue> {
    Some(match drawn(spec, value) {
        Drawn::Number(x) => ParamValue::Number(x),
        Drawn::Bool(b) => ParamValue::Bool(b),
        Drawn::Choice(i) => match &spec.kind {
            ParamKind::Choice { options, .. } => ParamValue::Text(options.get(i as usize)?.clone()),
            _ => return None,
        },
        Drawn::Point(p) => ParamValue::Point(p),
        Drawn::Color(_) => match (value, &spec.kind) {
            (Some(ParamValue::Text(t)), _) if rgba_unit(t).is_some() => ParamValue::Text(t.clone()),
            (_, ParamKind::Color { default, .. }) => ParamValue::Text(default.clone()),
            _ => return None,
        },
        Drawn::Other(v) => return v.cloned(),
    })
}

/// `x` within `lo..=hi`, never panicking on a definition whose bounds are crossed or not numbers.
fn within(x: f64, lo: f64, hi: f64) -> f64 {
    x.max(lo).min(hi)
}

fn number(value: Option<&ParamValue>, default: f64) -> f64 {
    match value {
        Some(ParamValue::Number(n)) if n.is_finite() => *n,
        _ => default,
    }
}

fn text(value: Option<&ParamValue>) -> Option<&str> {
    match value {
        Some(ParamValue::Text(t)) => Some(t),
        _ => None,
    }
}

/// `#RRGGBB` or `#RRGGBBAA` as straight RGBA 0–1.
pub(crate) fn rgba_unit(hex: &str) -> Option<[f32; 4]> {
    let h = hex.strip_prefix('#')?;
    if !(h.len() == 6 || h.len() == 8) || !h.is_ascii() {
        return None;
    }
    let byte = |i: usize| {
        u8::from_str_radix(&h[i..i + 2], 16)
            .ok()
            .map(|b| f32::from(b) / 255.0)
    };
    Some([
        byte(0)?,
        byte(2)?,
        byte(4)?,
        if h.len() == 8 { byte(6)? } else { 1.0 },
    ])
}

#[cfg(test)]
mod tests;
