//! SVG path data (`d`) as absolute commands: M, L, H, V, C, S, Q, T, A and Z, relative and absolute, with arcs turned
//! into cubic curves (SVG 1.1 implementation notes F.6). Finite numbers only, at most
//! [`SceneLimits::MAX_PATH_COMMANDS`](super::SceneLimits::MAX_PATH_COMMANDS) commands; total on any input.

use super::SceneLimits;

mod arc;
mod bounds;

pub use bounds::bounds;
pub(crate) use bounds::{cubic_at, quad_at};

/// The largest coordinate a path may hold, so the arithmetic on it stays finite.
pub const MAX_COORD: f64 = 1e7;

/// One absolute drawing command.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCmd {
    Move {
        x: f64,
        y: f64,
    },
    Line {
        x: f64,
        y: f64,
    },
    Quad {
        x1: f64,
        y1: f64,
        x: f64,
        y: f64,
    },
    Cubic {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        x: f64,
        y: f64,
    },
    Close,
}

/// `d` as absolute commands, or why it can't be read (in English, for the agent).
pub fn parse_d(d: &str) -> Result<Vec<PathCmd>, String> {
    Parser {
        s: d.as_bytes(),
        i: 0,
        out: Vec::new(),
    }
    .run()
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    out: Vec<PathCmd>,
}

impl Parser<'_> {
    fn run(mut self) -> Result<Vec<PathCmd>, String> {
        let (mut cx, mut cy) = (0.0, 0.0);
        let (mut sx, mut sy) = (0.0, 0.0);
        // The previous curve's last control point, for S and T.
        let mut last_cubic: Option<(f64, f64)> = None;
        let mut last_quad: Option<(f64, f64)> = None;
        let mut cmd: Option<u8> = None;
        loop {
            self.skip();
            let Some(&c) = self.s.get(self.i) else { break };
            if c.is_ascii_alphabetic() {
                self.i += 1;
                cmd = Some(c);
            } else {
                // Numbers repeat the command before; after a move they draw lines.
                cmd = match cmd {
                    None => return Err("Path data starts with M (a move).".into()),
                    Some(b'Z' | b'z') => return Err(format!("Numbers follow Z at {}.", self.i)),
                    Some(b'M') => Some(b'L'),
                    Some(b'm') => Some(b'l'),
                    other => other,
                };
            }
            let Some(c) = cmd else { break };
            if self.out.is_empty() && !matches!(c, b'M' | b'm') {
                return Err("Path data starts with M (a move).".into());
            }
            let rel = c.is_ascii_lowercase();
            let (ox, oy) = if rel { (cx, cy) } else { (0.0, 0.0) };
            let (mut cubic, mut quad) = (None, None);
            match c.to_ascii_uppercase() {
                b'M' => {
                    let (x, y) = (ox + self.num()?, oy + self.num()?);
                    self.push(PathCmd::Move { x, y })?;
                    (cx, cy, sx, sy) = (x, y, x, y);
                }
                b'L' => {
                    let (x, y) = (ox + self.num()?, oy + self.num()?);
                    self.push(PathCmd::Line { x, y })?;
                    (cx, cy) = (x, y);
                }
                b'H' => {
                    let x = ox + self.num()?;
                    self.push(PathCmd::Line { x, y: cy })?;
                    cx = x;
                }
                b'V' => {
                    let y = oy + self.num()?;
                    self.push(PathCmd::Line { x: cx, y })?;
                    cy = y;
                }
                b'C' | b'S' => {
                    let (x1, y1) = if c.eq_ignore_ascii_case(&b'C') {
                        (ox + self.num()?, oy + self.num()?)
                    } else {
                        // The reflection of the previous cubic's second control point, else the current point.
                        last_cubic.map_or((cx, cy), |(px, py)| (2.0 * cx - px, 2.0 * cy - py))
                    };
                    let (x2, y2) = (ox + self.num()?, oy + self.num()?);
                    let (x, y) = (ox + self.num()?, oy + self.num()?);
                    self.push(PathCmd::Cubic {
                        x1,
                        y1,
                        x2,
                        y2,
                        x,
                        y,
                    })?;
                    cubic = Some((x2, y2));
                    (cx, cy) = (x, y);
                }
                b'Q' | b'T' => {
                    let (x1, y1) = if c.eq_ignore_ascii_case(&b'Q') {
                        (ox + self.num()?, oy + self.num()?)
                    } else {
                        last_quad.map_or((cx, cy), |(px, py)| (2.0 * cx - px, 2.0 * cy - py))
                    };
                    let (x, y) = (ox + self.num()?, oy + self.num()?);
                    self.push(PathCmd::Quad { x1, y1, x, y })?;
                    quad = Some((x1, y1));
                    (cx, cy) = (x, y);
                }
                b'A' => {
                    let (rx, ry, turn) = (self.num()?, self.num()?, self.num()?);
                    let (large, sweep) = (self.flag()?, self.flag()?);
                    let (x, y) = (ox + self.num()?, oy + self.num()?);
                    self.arc((cx, cy), (rx, ry), turn, large, sweep, (x, y))?;
                    (cx, cy) = (x, y);
                }
                b'Z' => {
                    self.push(PathCmd::Close)?;
                    (cx, cy) = (sx, sy);
                }
                _ => {
                    return Err(format!(
                        "Path data has no command {:?}: it takes M, L, H, V, C, S, Q, T, A and Z.",
                        char::from(c)
                    ))
                }
            }
            (last_cubic, last_quad) = (cubic, quad);
            if !(cx.is_finite() && cy.is_finite()) {
                return Err("Path data moves too far.".into());
            }
        }
        Ok(self.out)
    }

    fn push(&mut self, cmd: PathCmd) -> Result<(), String> {
        if self.out.len() >= SceneLimits::MAX_PATH_COMMANDS {
            return Err(format!(
                "A path has at most {} commands (an arc counts as up to four curves).",
                SceneLimits::MAX_PATH_COMMANDS
            ));
        }
        self.out.push(cmd);
        Ok(())
    }

    /// Whitespace and commas.
    fn skip(&mut self) {
        while self
            .s
            .get(self.i)
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b',')
        {
            self.i += 1;
        }
    }

    /// One finite number of at most `MAX_COORD`: an optional sign, digits with an optional point, an exponent.
    fn num(&mut self) -> Result<f64, String> {
        self.skip();
        let start = self.i;
        let at = |i: usize| self.s.get(i).copied();
        let mut i = start;
        if matches!(at(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let mut digits = 0;
        while at(i).is_some_and(|b| b.is_ascii_digit()) {
            i += 1;
            digits += 1;
        }
        if at(i) == Some(b'.') {
            i += 1;
            while at(i).is_some_and(|b| b.is_ascii_digit()) {
                i += 1;
                digits += 1;
            }
        }
        if digits > 0 && matches!(at(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(at(j), Some(b'+' | b'-')) {
                j += 1;
            }
            if at(j).is_some_and(|b| b.is_ascii_digit()) {
                while at(j).is_some_and(|b| b.is_ascii_digit()) {
                    j += 1;
                }
                i = j;
            }
        }
        let value = self
            .s
            .get(start..i)
            .filter(|_| digits > 0)
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(|t| t.parse::<f64>().ok())
            .filter(|v| v.is_finite() && v.abs() <= MAX_COORD);
        match value {
            Some(v) => {
                self.i = i;
                Ok(v)
            }
            None => Err(format!(
                "Path data needs a finite number (at most {MAX_COORD:e}) at {start}."
            )),
        }
    }

    /// An arc flag: `0` or `1`, which may touch the next number.
    fn flag(&mut self) -> Result<bool, String> {
        self.skip();
        match self.s.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.i += 1;
                Ok(true)
            }
            _ => Err(format!("An arc's flags are 0 or 1 (at {}).", self.i)),
        }
    }
}
