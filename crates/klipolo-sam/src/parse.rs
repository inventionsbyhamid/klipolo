//! Reader for SAM 3.1's text output.
//!
//! Grammar, one line per frame (frames with nothing in them have no line, and
//! lines are not always in frame order):
//!
//! ```text
//! line   = "<" FRAME "f>" record ("," record)*
//! record = ID token+                     exactly one box and one mask token
//! box    = "<|box;x1=I;y1=I;x2=I;y2=I;w=I;h=I[;c=D]|>"   inclusive, source pixels
//! mask   = "<|mask;x=0;y=0[;c=D];data=H,W,<~ or !>PAYLOAD|>"
//! ```
//!
//! Mask payloads are base85 and are skipped, not decoded. Their alphabet leaves
//! out `|`, `;`, `,`, `<`, `>`, but a payload ends only at `|>`, so a stray `|`
//! or `;` would not break the reader either.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A box in source pixels, half-open: `x0 <= x < x1`, `y0 <= y < y1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxPx {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl BoxPx {
    pub fn width(&self) -> u32 {
        self.x1.saturating_sub(self.x0)
    }

    pub fn height(&self) -> u32 {
        self.y1.saturating_sub(self.y0)
    }
}

/// One tracked object on one frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Obj {
    pub id: u32,
    pub bbox: BoxPx,
    /// Source frame size the box is measured in (`w`, `h` of the box token).
    pub frame_w: u32,
    pub frame_h: u32,
    /// The box's `c`, present when the request set `include_confidence`.
    pub confidence: Option<f32>,
}

/// Every object SAM reported on one frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameObjects {
    pub frame: u32,
    pub objects: Vec<Obj>,
}

/// A line of SAM output that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("SAM output line {line}: {reason}")]
pub struct ParseError {
    /// 1-based line number in the output text.
    pub line: usize,
    pub reason: String,
}

/// Parses SAM's frame lines into boxes, sorted by frame.
///
/// Blank lines are skipped. Any other line that is not a well-formed frame line
/// fails the whole parse. If a frame appears on two lines, the later line's
/// objects replace earlier ones with the same id.
pub fn parse_frames(text: &str) -> Result<Vec<FrameObjects>, ParseError> {
    let mut frames: BTreeMap<u32, Vec<Obj>> = BTreeMap::new();
    for (index, raw) in text.split('\n').enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let (frame, objects) = parse_line(line).map_err(|reason| ParseError {
            line: index + 1,
            reason,
        })?;
        let slot = frames.entry(frame).or_default();
        for obj in objects {
            match slot.iter_mut().find(|o| o.id == obj.id) {
                Some(existing) => *existing = obj,
                None => slot.push(obj),
            }
        }
    }
    Ok(frames
        .into_iter()
        .map(|(frame, objects)| FrameObjects { frame, objects })
        .collect())
}

fn parse_line(line: &str) -> Result<(u32, Vec<Obj>), String> {
    let header = "expected a frame header like <12f>";
    let rest = line.strip_prefix('<').ok_or(header)?;
    let (digits, rest) = split_digits(rest);
    let frame = parse_u32(digits).ok_or(header)?;
    let rest = rest.strip_prefix('f').ok_or(header)?;
    // The header has no fields today; skip any `;key=value` the API adds later.
    let mut rest = match rest.strip_prefix('>') {
        Some(body) => body,
        None if rest.starts_with(';') => {
            let close = rest.find('>').ok_or(header)?;
            &rest[close + 1..]
        }
        None => return Err(header.into()),
    };

    let mut objects: Vec<Obj> = Vec::new();
    while !rest.is_empty() {
        let (obj, after) = parse_record(rest)?;
        if objects.iter().any(|o| o.id == obj.id) {
            return Err(format!("object {} appears twice", obj.id));
        }
        objects.push(obj);
        rest = match after.strip_prefix(',') {
            Some("") => return Err("line ends with a comma".into()),
            Some(next) => next,
            None => after,
        };
    }
    Ok((frame, objects))
}

/// Reads one `ID<|...|><|...|>` record and returns what follows it.
fn parse_record(text: &str) -> Result<(Obj, &str), String> {
    let (digits, mut rest) = split_digits(text);
    let id = parse_u32(digits).ok_or("expected an object id")?;

    let mut box_fields = None;
    let mut mask_fields = None;
    while let Some(open) = rest.strip_prefix("<|") {
        let close = open
            .find("|>")
            .ok_or_else(|| format!("object {id} has an unclosed token"))?;
        let token = &open[..close];
        rest = &open[close + 2..];
        let (name, fields) = token.split_once(';').unwrap_or((token, ""));
        let slot = match name {
            "box" => &mut box_fields,
            "mask" => &mut mask_fields,
            _ => continue,
        };
        if slot.replace(fields).is_some() {
            return Err(format!("object {id} has two {name} tokens"));
        }
    }

    let box_fields = box_fields.ok_or_else(|| format!("object {id} has no box"))?;
    let mask_fields = mask_fields.ok_or_else(|| format!("object {id} has no mask"))?;
    check_mask(mask_fields).map_err(|reason| format!("object {id}: {reason}"))?;
    let (bbox, frame_w, frame_h, confidence) =
        read_box(box_fields).map_err(|reason| format!("object {id}: {reason}"))?;
    let obj = Obj {
        id,
        bbox,
        frame_w,
        frame_h,
        confidence,
    };
    Ok((obj, rest))
}

const BOX_KEYS: [&str; 7] = ["x1", "y1", "x2", "y2", "w", "h", "c"];

/// Reads `x1;y1;x2;y2;w;h[;c]` and turns the inclusive corners half-open.
fn read_box(fields: &str) -> Result<(BoxPx, u32, u32, Option<f32>), String> {
    let mut values: [Option<&str>; 7] = [None; 7];
    for field in fields.split(';').map(str::trim).filter(|f| !f.is_empty()) {
        let (key, value) = field
            .split_once('=')
            .ok_or_else(|| format!("box field {field} has no value"))?;
        let key = key.trim();
        let Some(slot) = BOX_KEYS.iter().position(|k| *k == key) else {
            continue;
        };
        if values[slot].replace(value.trim()).is_some() {
            return Err(format!("box field {key} appears twice"));
        }
    }

    let int = |slot: usize| -> Result<u32, String> {
        let key = BOX_KEYS[slot];
        let value = values[slot].ok_or_else(|| format!("box has no {key}"))?;
        parse_u32(value).ok_or_else(|| format!("box {key}={value} is not a whole number"))
    };
    let (left, top, right, bottom) = (int(0)?, int(1)?, int(2)?, int(3)?);
    let (w, h) = (int(4)?, int(5)?);
    if w == 0 || h == 0 || left > right || top > bottom || right >= w || bottom >= h {
        return Err(format!(
            "box {left},{top},{right},{bottom} does not fit a {w}x{h} frame"
        ));
    }

    let confidence = match values[6] {
        None => None,
        Some(text) => match text.parse::<f32>() {
            Ok(c) if c.is_finite() && (0.0..=1.0).contains(&c) => Some(c),
            _ => return Err(format!("box confidence {text} is not a number from 0 to 1")),
        },
    };

    let bbox = BoxPx {
        x0: left,
        y0: top,
        x1: right + 1,
        y1: bottom + 1,
    };
    Ok((bbox, w, h, confidence))
}

/// Checks `data=H,W,<marker>payload` without decoding the payload.
///
/// `data` is the mask's last field, so everything after `data=` belongs to it.
fn check_mask(fields: &str) -> Result<(), String> {
    let data = match fields.strip_prefix("data=") {
        Some(data) => data,
        None => {
            let at = fields.find(";data=").ok_or("mask has no data")?;
            &fields[at + ";data=".len()..]
        }
    };
    let mut parts = data.splitn(3, ',');
    let height = parts.next().and_then(parse_u32);
    let width = parts.next().and_then(parse_u32);
    match (height, width) {
        (Some(h), Some(w)) if h > 0 && w > 0 => {}
        _ => return Err("mask size is not H,W".into()),
    }
    match parts.next().and_then(|payload| payload.chars().next()) {
        Some('~' | '!') => Ok(()),
        _ => Err("mask payload does not start with ~ or !".into()),
    }
}

fn split_digits(text: &str) -> (&str, &str) {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    text.split_at(end)
}

/// Plain ASCII digits only (no sign), within `u32`.
fn parse_u32(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}
