//! Glyph sets: a text that changes as it plays is laid out once, when its instance is built, as
//! the glyphs it can show. A counter's digits (on tabular figures, `tnum`, when its face has them), its separators,
//! prefix and suffix; a scramble's alphabet beside its final strings (each laid out whole, as every hold's is). A
//! frame only places glyphs from the set (`compose`): a counter's number right-aligned on tabular advances at the edge
//! its widest value reaches, a scramble's random letters centred where the final ones stand. Composing shapes,
//! rasterises and allocates nothing.

use arrayvec::ArrayString;
use klipolo_fonts::index::FontIndex;
use klipolo_model::scene::instance::CounterText;

use super::text::{CTextNode, NO_UNIT, Options, TextUnits, piece_text};
use super::text_units::Grain;
use crate::render::shared_fonts::SharedFonts;

/// The glyphs a counter's number is written with, in this order after its prefix.
const NUMBER_GLYPHS: &str = "0123456789,.-";

/// A text whose glyphs change as it plays.
pub enum GlyphSet {
    Counter(CounterSet),
    Scramble(ScrambleSet),
}

impl GlyphSet {
    /// The set's own layout (its glyphs' pieces).
    pub fn units(&self) -> &TextUnits {
        match self {
            Self::Counter(c) => &c.units,
            Self::Scramble(s) => &s.units,
        }
    }
}

/// Where one glyph of a frame goes: piece `piece` (of the set's own layout, or of the node's shown text when
/// `of_text`), moved by `dx`, `dy`, with its split unit and typewriter step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub piece: usize,
    pub of_text: bool,
    pub dx: f32,
    pub dy: f32,
    pub unit: u16,
    pub reveal: u16,
}

/// One grapheme of a counter's glyph line: where it starts, its advance and its piece (`NO_UNIT`: whitespace).
#[derive(Clone, Copy, Debug)]
struct Slot {
    x: f32,
    advance: f32,
    piece: u16,
}

pub struct CounterSet {
    /// Its prefix, the number's glyphs and its suffix on one line, at the fit of its widest value.
    pub units: TextUnits,
    prefix: String,
    suffix: String,
    slots: Vec<Slot>,
    prefix_slots: usize,
    /// A digit's advance (tabular: every digit's slot is as wide as the widest digit).
    digit: f32,
    /// The right edge every value ends at: where its widest one ends, aligned in the box.
    right: f32,
}

pub struct ScrambleSet {
    /// One piece per letter of its alphabet.
    pub units: TextUnits,
    letters: Vec<(char, u16)>,
}

impl CounterSet {
    /// `c` of `node` laid out once in a box of `box_px` pixels (`px` a 1080p pixel): its widest value fitted as the
    /// node fits, then its glyph line at that size.
    pub(super) fn build(
        fonts: &mut SharedFonts,
        index: &FontIndex,
        node: &CTextNode,
        c: CounterText,
        box_px: (f32, f32),
        px: f32,
        base: Options,
    ) -> Self {
        let mut widest = ArrayString::<64>::new();
        for v in [c.from, c.to] {
            let mut s = ArrayString::<64>::new();
            c.format.write(v, &mut s);
            if s.chars().count() > widest.chars().count() {
                widest = s;
            }
        }
        let opts = Options {
            grain: Some(Grain::Chars),
            scale: None,
            tabular: true,
            ..base
        };
        let line = format!("{}{}{}", c.prefix, widest, c.suffix);
        let sized = TextUnits::lay_out_parts(fonts, index, node, &[(&line, 0)], box_px, px, opts);
        let right = sized.lines.first().map_or(box_px.0, |l| l[2]);
        let glyphs = format!("{}{NUMBER_GLYPHS}{}", c.prefix, c.suffix);
        let opts = Options {
            scale: Some(sized.scale),
            ..opts
        };
        let mut units =
            TextUnits::lay_out_parts(fonts, index, node, &[(&glyphs, 0)], box_px, px, opts);
        units.notes.join(sized.notes);
        let slots = slots_of(&units);
        let prefix_slots = c.prefix.chars().count().min(slots.len());
        let digit = (0..10)
            .filter_map(|d| slots.get(prefix_slots + d))
            .map(|s| s.advance)
            .fold(0.0, f32::max);
        Self {
            units,
            prefix: c.prefix.to_string(),
            suffix: c.suffix.to_string(),
            slots,
            prefix_slots,
            digit,
            right,
        }
    }

    /// The glyphs of `shown` (a frame's counter text: its prefix, number and suffix), right-aligned at the widest
    /// value's edge, each to `put`.
    pub fn compose(&self, shown: &str, mut put: impl FnMut(Placed)) {
        let mid = shown
            .strip_prefix(self.prefix.as_str())
            .map_or(shown, |r| r.strip_suffix(self.suffix.as_str()).unwrap_or(r));
        let number = |c: char| match c {
            '0'..='9' => Some(c as usize - '0' as usize),
            ',' => Some(10),
            '.' => Some(11),
            '-' => Some(12),
            _ => None,
        };
        let suffix_from = self.prefix_slots + NUMBER_GLYPHS.len();
        let width = |s: &Slot, digit: bool| if digit { self.digit } else { s.advance };
        let (affixes, numbers): (f32, f32) = (
            self.slots[..self.prefix_slots]
                .iter()
                .chain(&self.slots[suffix_from.min(self.slots.len())..])
                .map(|s| s.advance)
                .sum(),
            mid.chars()
                .filter_map(number)
                .filter_map(|k| {
                    self.slots
                        .get(self.prefix_slots + k)
                        .map(|s| width(s, k < 10))
                })
                .sum(),
        );
        let mut x = self.right - affixes - numbers;
        let mut unit = 0u16;
        let mut place = |slot: &Slot, w: f32, put: &mut dyn FnMut(Placed)| {
            if slot.piece != NO_UNIT {
                put(Placed {
                    piece: usize::from(slot.piece),
                    of_text: false,
                    dx: x + (w - slot.advance) / 2.0 - slot.x,
                    dy: 0.0,
                    unit,
                    reveal: unit,
                });
                unit = unit.saturating_add(1);
            }
            x += w;
        };
        for s in &self.slots[..self.prefix_slots] {
            place(s, s.advance, &mut put);
        }
        for k in mid.chars().filter_map(number) {
            if let Some(s) = self.slots.get(self.prefix_slots + k) {
                place(s, width(s, k < 10), &mut put);
            }
        }
        for s in self.slots.get(suffix_from..).unwrap_or(&[]) {
            place(s, s.advance, &mut put);
        }
    }
}

/// Each character of a one-line layout: where it starts, its advance, its piece (a cluster's are its first
/// character's; the rest stay empty).
fn slots_of(units: &TextUnits) -> Vec<Slot> {
    let count = units.text.chars().count();
    let mut slots = vec![
        Slot {
            x: f32::MAX,
            advance: 0.0,
            piece: NO_UNIT,
        };
        count
    ];
    let mut slot_of = vec![0usize; units.text.len() + 1];
    for (k, (at, _)) in units.text.char_indices().enumerate() {
        slot_of[at] = k;
    }
    for glyph in &units.glyphs {
        if let Some(s) = slots.get_mut(slot_of.get(glyph.start).copied().unwrap_or(usize::MAX)) {
            s.x = s.x.min(glyph.x);
            s.advance += glyph.w;
        }
    }
    for (k, p) in units.pieces.iter().enumerate() {
        if let Some(s) = slots.get_mut(
            slot_of
                .get(p.bytes[0] as usize)
                .copied()
                .unwrap_or(usize::MAX),
        ) {
            s.piece = k.min(usize::from(NO_UNIT) - 1) as u16;
        }
    }
    for s in &mut slots {
        if s.x == f32::MAX {
            s.x = 0.0;
        }
    }
    slots
}

impl ScrambleSet {
    /// `letters` laid out once, one piece each, at `scale` (the node's text's fit), within `base`'s room and size.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build(
        fonts: &mut SharedFonts,
        index: &FontIndex,
        node: &CTextNode,
        letters: &[char],
        scale: f32,
        box_px: (f32, f32),
        px: f32,
        base: Options,
    ) -> Self {
        let alphabet: String = letters.iter().filter(|c| !c.is_whitespace()).collect();
        let opts = Options {
            room: base.room.min(64),
            grain: Some(Grain::Chars),
            scale: Some(scale),
            tabular: false,
            ..base
        };
        let units =
            TextUnits::lay_out_parts(fonts, index, node, &[(&alphabet, 0)], box_px, px, opts);
        let letters = units
            .pieces
            .iter()
            .enumerate()
            .filter_map(|(k, p)| Some((piece_text(&units, p).chars().next()?, k as u16)))
            .collect();
        Self { units, letters }
    }

    /// The glyphs of `shown` (a frame's scramble of `text`, character for character): each settled letter `text`'s own
    /// piece, each random one the alphabet's, centred where `text`'s stands, to `put`.
    pub fn compose(&self, text: &TextUnits, shown: &str, mut put: impl FnMut(Placed)) {
        let mut chars = shown.chars();
        let mut at = 0u32;
        for (k, p) in text.pieces.iter().enumerate() {
            while at < p.chars_at {
                if chars.next().is_none() {
                    return;
                }
                at += 1;
            }
            let own = piece_text(text, p);
            let mut same = true;
            let mut first = None;
            for c in own.chars() {
                let b = chars.next();
                at += 1;
                first = first.or(b);
                same &= b == Some(c);
            }
            let placed = |piece, of_text, dx, dy| Placed {
                piece,
                of_text,
                dx,
                dy,
                unit: p.unit,
                reveal: p.reveal,
            };
            if same {
                put(placed(k, true, 0.0, 0.0));
            } else if let Some(&(_, a)) =
                first.and_then(|c| self.letters.iter().find(|(l, _)| *l == c))
            {
                let q = &self.units.pieces[usize::from(a)];
                let centre = |r: [f32; 4]| (r[0] + r[2]) / 2.0;
                put(placed(
                    usize::from(a),
                    false,
                    centre(p.rect) - centre(q.rect),
                    p.baseline - q.baseline,
                ));
            }
        }
    }
}
