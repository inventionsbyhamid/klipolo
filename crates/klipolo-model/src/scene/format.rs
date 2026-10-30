//! The counter's number format (video content, so no locale): `0`, `#,##0`, `0.0`, `#,##0.00`, at most 6 decimals,
//! commas between thousands when the pattern has one (left out when they would take it past 24 characters). Numbers
//! are held within ±1e15 (the check's counter range) and written into fixed buffers, never allocating, so a counter can
//! be formatted every frame. A number is one word: it never holds a space (the renderer's split by words agrees).

use std::fmt::Write;

use arrayvec::ArrayString;

/// The farthest a counter counts either way.
pub const MAX_COUNT: f64 = 1e15;
/// The most characters a number shown as text takes (`$i`, `$row.v0`, a number in a text key).
pub const NUMBER_CHARS: usize = 24;

/// How a counter writes its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumberFormat {
    /// Commas between thousands.
    pub group: bool,
    /// Digits after the point, 0–6.
    pub decimals: u8,
    /// Digits before the point at least, 1–16.
    pub min_int: u8,
}

impl NumberFormat {
    pub const PLAIN: Self = Self {
        group: false,
        decimals: 0,
        min_int: 1,
    };

    /// `#,##0.00`: a comma groups; the `0`s before the point are the least digits shown, the `0`s or `#`s after it
    /// the decimals. Anything else in the pattern is ignored.
    pub fn parse(pattern: &str) -> Self {
        let (int, frac) = pattern.split_once('.').unwrap_or((pattern, ""));
        let zeros = int.bytes().filter(|&b| b == b'0').count();
        let decimals = frac.bytes().filter(|&b| b == b'0' || b == b'#').count();
        Self {
            group: int.contains(','),
            decimals: decimals.min(6) as u8,
            min_int: zeros.clamp(1, 16) as u8,
        }
    }

    /// Writes `v` (held within ±[`MAX_COUNT`]; anything not finite is 0) as far as `out` has room.
    pub fn write<const N: usize>(&self, v: f64, out: &mut ArrayString<N>) {
        let v = if v.is_finite() {
            v.clamp(-MAX_COUNT, MAX_COUNT)
        } else {
            0.0
        };
        let decimals = usize::from(self.decimals.min(6));
        // 16 digits, the point and 6 decimals fit.
        let mut digits = ArrayString::<40>::new();
        let _ = write!(digits, "{:.*}", decimals, v.abs());
        let (int, frac) = digits.split_once('.').unwrap_or((digits.as_str(), ""));
        let zero = digits.bytes().all(|b| b == b'0' || b == b'.');
        let sign = v < 0.0 && !zero;
        if sign {
            push(out, '-');
        }
        let pad = usize::from(self.min_int).saturating_sub(int.len());
        let width = int.len() + pad;
        // At most `NUMBER_CHARS`, as the check counts a counter: the commas go first. Without them
        // the widest number fits (a sign, 16 digits, the point and 6 decimals).
        let commas = width.saturating_sub(1) / 3;
        let point = if decimals > 0 { decimals + 1 } else { 0 };
        let group = self.group && usize::from(sign) + width + commas + point <= NUMBER_CHARS;
        for i in 0..width {
            let digit = if i < pad {
                '0'
            } else {
                char::from(int.as_bytes()[i - pad])
            };
            if group && i > 0 && (width - i) % 3 == 0 {
                push(out, ',');
            }
            push(out, digit);
        }
        if decimals > 0 {
            push(out, '.');
            for c in frac.chars() {
                push(out, c);
            }
        }
    }
}

/// Pushes one character if it fits.
pub(crate) fn push<const N: usize>(out: &mut ArrayString<N>, c: char) {
    let _ = out.try_push(c);
}

/// A number shown as text, in at most [`NUMBER_CHARS`] characters: as written when that fits (`3`, `0.25`), else in
/// exponent form (`1e300`, `1.2345678901234567e-300`).
pub fn number_text(v: f64) -> ArrayString<NUMBER_CHARS> {
    let v = if v.is_finite() { v } else { 0.0 };
    let v = if v == 0.0 { 0.0 } else { v };
    let mut out = ArrayString::<NUMBER_CHARS>::new();
    if write!(out, "{v}").is_err() {
        out.clear();
        let _ = write!(out, "{v:e}");
    }
    out
}
