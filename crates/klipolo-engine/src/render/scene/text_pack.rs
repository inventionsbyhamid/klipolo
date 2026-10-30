//! A frame's text as draws: each piece of the text a node shows this frame (its own, a hold's, or a
//! counter's or a scramble's composed from its glyph set) as a quad from its atlas cell, moved by its split unit (about
//! the unit's middle), by an animated size (about the block's anchor), letter spacing and line height (glyph by glyph
//! and line by line), placed in its node's box (`text_move.rs`); the pieces a typewriter hasn't reached left out; each
//! span background under its pieces, as wide as they stand now. What a node's pieces of one look share (their paint,
//! clips and baked shadow) goes once into the frame's paints, which each piece names, and its text is
//! placed once a frame, not per piece. Pure arithmetic into lists whose room was made with the instance's text:
//! it shapes, rasterises and allocates nothing. The text was laid out for the layer's largest scale:
//! it is fitted to the node's box as this frame has it, evenly (`text_fit.rs`), so it scales with its card, and shifted
//! by its rest pose's rounding, so it rests on whole pixels and glides when it moves.

use klipolo_model::scene::eval::{NodeState, TextNow, UnitState};
use klipolo_model::scene::modifiers::reveal_count;

use super::glyph_sets::{GlyphSet, Placed};
use super::place::World;
use super::statics::NodeStatic;
use super::text::{NO_UNIT, TextUnits};
use super::text_atlas::Cell;
use super::text_backgrounds::backgrounds;
pub(super) use super::text_blur::BlurBudget;
use super::text_blur::MAX_BLUR_PX;
use super::text_layouts::TextLayouts;
use super::text_move::{Placing, moved};
use super::text_nodes::{NOT_LAID, NodeText};
use super::text_paint::paint_entry;
use super::text_pipeline::{TextDraw, TextPaintDraw};

/// What a frame's text node is drawn with: its place, its state and rest, its units, the text it shows.
pub(super) struct NodeFrame<'a> {
    /// The instance node it draws.
    pub node: usize,
    pub here: &'a World,
    pub s: &'a NodeState,
    pub rest: &'a NodeState,
    pub st: &'a NodeStatic,
    pub units: &'a [UnitState],
    /// A counter's or a scramble's text this frame.
    pub buffer: &'a str,
    /// Its place at its rest pose, nothing moving (what its whole-pixel shift is measured at).
    pub rest_here: &'a World,
    /// Target pixels per 1080p pixel the text was laid out at (the layer's largest scale: `TextLayouts::k`).
    pub k: f64,
    /// The instance's spans' gradients (`statics`' third part).
    pub spans: &'a [i32],
    /// The frame's blurring units, held to `MAX_BLURRING_UNITS`, and the atlas's size in texels (where they blur).
    pub blur: &'a BlurBudget,
    pub atlas: (u32, u32),
    /// Where the colour plane lies in the atlas (`TextLayouts::cmap`).
    pub cmap: [f32; 4],
}

/// Where a frame's text goes: its draws, and the paints they name, at most as many as its text's GPU
/// buffers hold (`room`: `TextLayouts::most` and `paints`).
pub(super) struct TextOut<'a> {
    pub draws: &'a mut Vec<TextDraw>,
    pub paints: &'a mut Vec<TextPaintDraw>,
    pub room: (usize, usize),
}

impl TextOut<'_> {
    /// Whether another draw, or another paint, would be past the room (or need an allocation).
    pub(super) fn full(&self) -> bool {
        let (draws, paints) = (self.draws.len(), self.paints.len());
        draws >= self.room.0.min(self.draws.capacity())
            || paints >= self.room.1.min(self.paints.capacity())
    }
}

/// Instance node `i`'s text this frame (`f`) as draws appended to `out`.
pub(super) fn pack_node(out: &mut TextOut, entry: &TextLayouts, i: usize, f: &NodeFrame) {
    let Some(nt) = entry.node(i) else {
        return;
    };
    if f.here.opacity <= 0.0 {
        return;
    }
    let set_cells = nt.cells.last().copied().unwrap_or(0) as usize;
    let cells = |layout: usize| nt.cells.get(layout).copied().unwrap_or(0) as usize;
    let from = |k: usize| &entry.cells[k.min(entry.cells.len())..];
    match (f.s.text, &nt.set) {
        (TextNow::Buffer(_), Some(GlyphSet::Counter(c))) => {
            let mut at = Shown::new(&c.units, from(set_cells), false, nt, f);
            c.compose(f.buffer, |p| at.piece(out, nt, f, p));
        }
        (TextNow::Buffer(t), Some(GlyphSet::Scramble(sc))) => {
            let Some(t) = layout_of(nt, usize::from(t)) else {
                return;
            };
            let Some(text) = nt.layouts.get(t) else {
                return;
            };
            let mut own = Shown::new(text, from(cells(t)), true, nt, f);
            let mut set = Shown::new(&sc.units, from(set_cells), false, nt, f);
            sc.compose(text, f.buffer, |p| {
                let shown = if p.of_text { &mut own } else { &mut set };
                shown.piece(out, nt, f, p)
            });
        }
        (now, _) => {
            let shown = match now {
                TextNow::Text(t) => usize::from(t),
                _ => 0,
            };
            let Some(t) = layout_of(nt, shown) else {
                return;
            };
            let Some(text) = nt.layouts.get(t) else {
                return;
            };
            let mut shown = Shown::new(text, from(cells(t)), true, nt, f);
            // Its baked shadow: this layout's one piece's shadow cell.
            shown.shadow = nt
                .shadow
                .as_ref()
                .and_then(|b| b.cells.get(t))
                .and_then(|&k| entry.shadows.get(k as usize))
                .copied();
            let steps = nt
                .reveal
                .then(|| reveal_count(f.s.reveal, text.reveal_units));
            if t == 0 && nt.paint.backgrounds.iter().any(Option::is_some) {
                backgrounds(out, nt, text, f, steps);
            }
            for (k, p) in text.pieces.iter().enumerate() {
                if steps.is_some_and(|n| p.reveal != NO_UNIT && usize::from(p.reveal) >= n) {
                    continue;
                }
                let placed = Placed {
                    piece: k,
                    of_text: true,
                    dx: 0.0,
                    dy: 0.0,
                    unit: p.unit,
                    reveal: p.reveal,
                };
                shown.piece(out, nt, f, placed);
            }
        }
    }
}

/// A layout drawn this frame: its cells; `units`: its split units' boxes are the units' (a glyph set's pieces each move
/// about their own middle); where it is placed this frame (once); its baked shadow's cell, if any; and the paint
/// its last piece named, by look (consecutive pieces of a look share one).
struct Shown<'a> {
    text: &'a TextUnits,
    cells: &'a [Cell],
    units: bool,
    at: Placing,
    shadow: Option<Cell>,
    last: Option<(u8, u32)>,
}

impl<'a> Shown<'a> {
    fn new(
        text: &'a TextUnits,
        cells: &'a [Cell],
        units: bool,
        nt: &NodeText,
        f: &NodeFrame,
    ) -> Self {
        Self {
            text,
            cells,
            units,
            at: Placing::of(nt, text, f),
            shadow: None,
            last: None,
        }
    }

    /// The paint pieces of look `look` name this frame: the last one's when it was of that look, else a new one.
    fn paint(&mut self, out: &mut TextOut, nt: &NodeText, f: &NodeFrame, look: u8) -> u32 {
        if let Some((l, k)) = self.last
            && l == look
        {
            return k;
        }
        let shadow = (self.shadow, self.at.u);
        let entry = paint_entry(nt, f, usize::from(look), self.text.clip, shadow);
        out.paints.push(entry);
        let k = (out.paints.len() - 1) as u32;
        self.last = Some((look, k));
        k
    }

    /// Piece `p` as a draw.
    fn piece(&mut self, out: &mut TextOut, nt: &NodeText, f: &NodeFrame, p: Placed) {
        let (Some(piece), Some(cell)) = (self.text.pieces.get(p.piece), self.cells.get(p.piece))
        else {
            return;
        };
        if out.full() {
            return;
        }
        let rect = shifted(piece.rect, p.dx, p.dy);
        let (text, units, at) = (self.text, self.units, &self.at);
        let (m, opacity) = moved(nt, text, piece, rect, f, p.unit, units, at);
        let opacity = f.here.opacity * opacity;
        if opacity <= 0.0 {
            return;
        }
        let paint = self.paint(out, nt, f, piece.look);
        // Its unit's blur and its node's, a Gaussian in box pixels, while the frame has room for it.
        let unit_blur = f
            .units
            .get(usize::from(p.unit))
            .filter(|_| p.unit != NO_UNIT)
            .map_or(0.0, |u| u.blur);
        // Clamped in 1080p pixels, then in box pixels (m8: a 4K frame blurs as its preview).
        let blur = ((f.s.blur + unit_blur).clamp(0.0, MAX_BLUR_PX) * f.k).min(256.0) as f32;
        let rect = shifted(cell.rect, p.dx, p.dy);
        let blurred = (blur > 0.0 && blur.is_finite() && f.blur.allow(f.node, p.unit))
            .then(|| f.blur.place(cell.uv, rect, blur, f.atlas))
            .flatten();
        let (grow, sigma, blur_a, blur_b) = blurred.map_or((0.0, 0.0, [0.0; 4], [0.0; 4]), |b| {
            (blur, b.sigma, b.a, b.b)
        });
        out.draws.push(TextDraw {
            m: [m[0], m[1], m[2], m[3]].map(|v| v as f32),
            t: [m[4] as f32, m[5] as f32, 0.0, 0.0],
            rect,
            uv: cell.uv,
            misc: [opacity as f32, paint as f32, grow, sigma],
            blur_a,
            blur_b,
        });
    }
}

/// The layout node `nt`'s text `shown` is drawn from: None for one past the room, or none.
fn layout_of(nt: &NodeText, shown: usize) -> Option<usize> {
    match nt.layout_of.get(shown).or(nt.layout_of.first()) {
        Some(&NOT_LAID) | None => None,
        Some(&k) => Some(usize::from(k)),
    }
}

fn shifted(r: [f32; 4], dx: f32, dy: f32) -> [f32; 4] {
    [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy]
}
