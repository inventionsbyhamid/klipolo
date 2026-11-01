//! A line's JSON into a checked [`Line`] (or a format-1 sample) through one flat, borrowed record: serde's tagged-enum
//! path buffers each line's fields first, this one allocates nothing for a position or a button.

use serde::Deserialize;

use super::{Capture, Line, MAX_LINE, SystemShape, TIME_LIMIT_US, VERSION};
use crate::shapes::{file_ok, hotspot_ok, parse_key, size_ok};
use crate::track::area_ok;
use crate::{ClickSource, Us};

/// A format-1 line (the first recorder builds): the cursor at one frame, `b` the buttons held as bits (1 the primary).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct V1 {
    pub t: Us,
    pub x: f32,
    pub y: f32,
    pub b: u8,
}

/// What a cursor line holds.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Parsed {
    Line(Line),
    V1(V1),
    /// A whole line of a known kind with an impossible value (the reader counts it): a time beyond `TIME_LIMIT_US`,
    /// an area whose size isn't positive, a shape whose height isn't positive or whose hotspot is outside its image.
    Refused,
}

/// Every field any line kind has; missing ones are None, unknown ones are ignored.
#[derive(Deserialize)]
struct Raw<'a> {
    #[serde(borrow)]
    k: Option<&'a str>,
    v: Option<u32>,
    capture: Option<Capture>,
    clicks: Option<ClickSource>,
    w: Option<f32>,
    h: Option<f32>,
    t: Option<Us>,
    x: Option<f32>,
    y: Option<f32>,
    btn: Option<u8>,
    d: Option<u8>,
    b: Option<u8>,
    id: Option<u16>,
    #[serde(borrow)]
    key: Option<&'a str>,
    #[serde(borrow)]
    file: Option<&'a str>,
    pw: Option<u16>,
    ph: Option<u16>,
    hx: Option<f32>,
    hy: Option<f32>,
    pts: Option<f32>,
    sys: Option<SystemShape>,
    on: Option<bool>,
}

fn finite(v: Option<f32>) -> Option<f32> {
    v.filter(|v| v.is_finite())
}

fn in_window(t: Us) -> bool {
    t.unsigned_abs() <= TIME_LIMIT_US.unsigned_abs()
}

/// One line without its newline (see the module notes of `format`).
pub(crate) fn parse(line: &[u8]) -> Option<Parsed> {
    if line.len() > MAX_LINE {
        return None;
    }
    let r: Raw = serde_json::from_slice(line).ok()?;
    let Some(k) = r.k else {
        let (t, x, y) = (r.t?, finite(r.x)?, finite(r.y)?);
        if !in_window(t) {
            return Some(Parsed::Refused);
        }
        return Some(Parsed::V1(V1 {
            t,
            x,
            y,
            b: r.b.unwrap_or(0),
        }));
    };
    let line = match k {
        "h" => Line::Header {
            v: r.v.filter(|&v| v == VERSION)?,
            capture: r.capture?,
            clicks: r.clicks?,
            w: finite(r.w)?,
            h: finite(r.h)?,
        },
        "m" => Line::Move {
            t: r.t?,
            x: finite(r.x)?,
            y: finite(r.y)?,
        },
        "b" => Line::Button {
            t: r.t?,
            btn: r.btn?,
            d: r.d.filter(|&d| d <= 1)?,
            x: finite(r.x)?,
            y: finite(r.y)?,
        },
        "S" => {
            let (key, file) = (r.key?, r.file?);
            let (pw, ph) = (r.pw?, r.ph?);
            if parse_key(key).is_none() || !file_ok(file) || !size_ok(pw, ph) {
                return None;
            }
            let (id, hx, hy, pts) = (r.id?, finite(r.hx)?, finite(r.hy)?, finite(r.pts)?);
            if pts <= 0.0 || !hotspot_ok((hx, hy), (pw, ph)) {
                return Some(Parsed::Refused);
            }
            Line::ShapeDef {
                id,
                key: key.to_owned(),
                file: file.to_owned(),
                pw,
                ph,
                hx,
                hy,
                pts,
                sys: r.sys,
            }
        }
        "s" => Line::Shape { t: r.t?, id: r.id? },
        "v" => Line::Visible { t: r.t?, on: r.on? },
        "a" => {
            let (t, w, h) = (r.t?, finite(r.w)?, finite(r.h)?);
            if !area_ok(w, h) {
                return Some(Parsed::Refused);
            }
            Line::Area { t, w, h }
        }
        "c" => Line::Clicks {
            t: r.t?,
            clicks: r.clicks?,
        },
        "x" => Line::Burned { t: r.t? },
        _ => return None,
    };
    let timed = !matches!(line, Line::Header { .. } | Line::ShapeDef { .. });
    if timed && r.t.is_some_and(|t| !in_window(t)) {
        return Some(Parsed::Refused);
    }
    Some(Parsed::Line(line))
}
