//! Reading the strings a scene's values may be: colours, percentages, pixels, clips and origins. Every number is
//! finite; anything else reads as None. Lengths inside clips and origins are ranged too: at most [`MAX_PX`] pixels and
//! [`MAX_FRACTION`] boxes either way.

/// The farthest a pixel position or length reaches, in 1080p pixels.
pub const MAX_PX: f64 = 100_000.0;
/// The farthest a fraction of a box reaches inside a clip or an origin (1,000 %), as `x` and `y` do.
pub const MAX_FRACTION: f64 = 10.0;

/// A length: a fraction of a box (`"50%"` is 0.5) or 1080p pixels (`"22px"`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Fraction(f64),
    Px(f64),
}

/// `clip`: `inset(top right bottom left round r)` or `circle(r at x y)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Clip {
    Inset {
        top: Length,
        right: Length,
        bottom: Length,
        left: Length,
        round: Length,
    },
    Circle {
        r: Length,
        x: Length,
        y: Length,
    },
}

/// A finite number.
fn number(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `#RGB`, `#RRGGBB`, `#RRGGBBAA`, `rgb(r, g, b)` or `rgba(r, g, b, a)` (also with spaces and `/ a`) as straight
/// RGBA, 0–1; `none` (SVG's) and `transparent` (CSS's) are clear, so a stroked path can go unfilled.
pub fn parse_color(s: &str) -> Option<[f32; 4]> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return hex_color(hex);
    }
    let lower = s.to_ascii_lowercase();
    if lower == "none" || lower == "transparent" {
        return Some([0.0; 4]);
    }
    let inner = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<&str> = inner
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    let channel = |p: &str| -> Option<f32> {
        let v = match p.strip_suffix('%') {
            Some(pct) => number(pct)? / 100.0,
            None => number(p)? / 255.0,
        };
        (0.0..=1.0).contains(&v).then_some(v as f32)
    };
    let alpha = |p: &str| -> Option<f32> {
        let v = match p.strip_suffix('%') {
            Some(pct) => number(pct)? / 100.0,
            None => number(p)?,
        };
        (0.0..=1.0).contains(&v).then_some(v as f32)
    };
    match parts[..] {
        [r, g, b] => Some([channel(r)?, channel(g)?, channel(b)?, 1.0]),
        [r, g, b, a] => Some([channel(r)?, channel(g)?, channel(b)?, alpha(a)?]),
        _ => None,
    }
}

fn hex_color(hex: &str) -> Option<[f32; 4]> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit =
        |i: usize| -> Option<u8> { hex.get(i..=i).and_then(|d| u8::from_str_radix(d, 16).ok()) };
    let byte =
        |i: usize| -> Option<f32> { Some(f32::from(digit(i)? * 16 + digit(i + 1)?) / 255.0) };
    let short = |i: usize| -> Option<f32> { Some(f32::from(digit(i)? * 17) / 255.0) };
    match hex.len() {
        3 => Some([short(0)?, short(1)?, short(2)?, 1.0]),
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 1.0]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

/// `"50%"` as 0.5.
pub fn parse_percent(s: &str) -> Option<f64> {
    number(s.trim().strip_suffix('%')?).map(|v| v / 100.0)
}

/// `"22px"` as 22.
pub fn parse_px(s: &str) -> Option<f64> {
    number(s.trim().strip_suffix("px")?)
}

/// `"50%"` or `"22px"`, within [`MAX_FRACTION`] and [`MAX_PX`]; a bare `0` is no length at all.
pub fn parse_length(s: &str) -> Option<Length> {
    let s = s.trim();
    if let Some(f) = parse_percent(s) {
        return (f.abs() <= MAX_FRACTION).then_some(Length::Fraction(f));
    }
    if let Some(px) = parse_px(s) {
        return (px.abs() <= MAX_PX).then_some(Length::Px(px));
    }
    (number(s) == Some(0.0)).then_some(Length::Px(0.0))
}

/// `inset(t r b l round r)` (one to four lengths, as in CSS) or `circle(r at x y)` (`at x y` defaulting to the
/// centre).
pub fn parse_clip(s: &str) -> Option<Clip> {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix("inset(").and_then(|r| r.strip_suffix(')')) {
        let (sides, round) = match inner.split_once("round") {
            Some((sides, r)) => (sides, parse_length(r)?),
            None => (inner, Length::Px(0.0)),
        };
        let v: Vec<Length> = sides
            .split_whitespace()
            .map(parse_length)
            .collect::<Option<_>>()?;
        let (top, right, bottom, left) = match v[..] {
            [a] => (a, a, a, a),
            [a, b] => (a, b, a, b),
            [a, b, c] => (a, b, c, b),
            [a, b, c, d] => (a, b, c, d),
            _ => return None,
        };
        return Some(Clip::Inset {
            top,
            right,
            bottom,
            left,
            round,
        });
    }
    let inner = s.strip_prefix("circle(")?.strip_suffix(')')?;
    let (r, at) = match inner.split_once(" at ") {
        Some((r, at)) => (r, Some(at)),
        None => (inner, None),
    };
    let r = parse_length(r)?;
    let (x, y) = match at {
        None => (Length::Fraction(0.5), Length::Fraction(0.5)),
        Some(at) => match at.split_whitespace().collect::<Vec<_>>()[..] {
            [x, y] => (parse_length(x)?, parse_length(y)?),
            _ => return None,
        },
    };
    Some(Clip::Circle { r, x, y })
}

/// One origin word or value: a fraction, `"50%"`, `"22px"`, or `left`, `center`, `right`, `top`, `bottom`. The
/// second value says which axis a word fixes (`Some(true)` for x).
fn origin_part(s: &str) -> Option<(Length, Option<bool>)> {
    Some(match s {
        "left" => (Length::Fraction(0.0), Some(true)),
        "right" => (Length::Fraction(1.0), Some(true)),
        "top" => (Length::Fraction(0.0), Some(false)),
        "bottom" => (Length::Fraction(1.0), Some(false)),
        "center" => (Length::Fraction(0.5), None),
        _ => match number(s) {
            Some(v) if v.abs() <= MAX_FRACTION => (Length::Fraction(v), None),
            Some(_) => return None,
            None => (parse_length(s)?, None),
        },
    })
}

/// A CSS `transform-origin` in the node's own box: `"50% 0%"`, `"center"`, `"left top"`, `"top"`, `"0.5 1"`.
pub fn parse_origin(s: &str) -> Option<(Length, Length)> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    let centre = Length::Fraction(0.5);
    match parts[..] {
        [one] => {
            let (v, axis) = origin_part(one)?;
            Some(if axis == Some(false) {
                (centre, v)
            } else {
                (v, centre)
            })
        }
        [a, b] => {
            let (va, axa) = origin_part(a)?;
            let (vb, axb) = origin_part(b)?;
            // CSS lets words come y first ("top left"); values always come x first.
            match (axa, axb) {
                (Some(false), Some(true)) | (Some(false), None) | (None, Some(true)) => {
                    Some((vb, va))
                }
                (Some(true), Some(true)) | (Some(false), Some(false)) => None,
                _ => Some((va, vb)),
            }
        }
        _ => None,
    }
}
