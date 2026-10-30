//! One instance's text nodes laid out: each text node's settings resolved (`CTextNode::of`), each text
//! it can show laid out whole (its own, its holds, text keys and tween texts), a counter's or a scramble's glyph set,
//! and its paint (spans' colours and backgrounds, its outline). An emoji image node is a text of one picture. Made when
//! its instance's text is built (`text_layouts.rs`), never per frame; every text laid out is charged to the node's 500
//! characters and the scene's 2,000, in paint order (`Room`).

use std::sync::Arc;

use klipolo_fonts::index::FontIndex;
use klipolo_model::font::FontRef;
use klipolo_model::scene::format::NUMBER_CHARS;
use klipolo_model::scene::instance::{Field, SceneInstance};
use klipolo_model::scene::{NodeType, RevealBy};

use super::glyph_sets::{CounterSet, GlyphSet, ScrambleSet};
use super::text::{
    Bindings, CTextNode, DEFAULT_FAMILY, NodeBindings, Options, TextLook, TextNotes, TextUnits,
    font_ref, own_look,
};
use super::text_paint::TextPaint;
use super::text_shadow::BakedShadow;
use super::text_units::Grain;
use crate::render::shared_fonts::SharedFonts;

/// One text node laid out: its texts by index (none for a counter), its glyph set, where each layout's cells start in
/// the atlas (then the set's), its paint, and the most draws a frame of it takes.
pub(super) struct NodeText {
    pub layouts: Vec<TextUnits>,
    /// Which layout each of its texts (`TextNow::Text(i)`) is drawn from, `NOT_LAID` for one past the room.
    pub layout_of: Vec<u16>,
    pub set: Option<GlyphSet>,
    pub cells: Vec<u32>,
    pub paint: TextPaint,
    pub most: usize,
    /// Its split units, laid out (what the instance places its tweens on).
    pub units: usize,
    pub reveal: bool,
    /// Its box when laid out, in target pixels: a frame whose box differs (px parts under motion, a stale size) is
    /// fitted to it.
    pub box_px: (f32, f32),
    /// Each look's letters' box as its own text lays them out (left, top, right, bottom): a span's gradient spans it.
    pub look_boxes: Vec<[f32; 4]>,
    /// Its shadow drawn from baked cells (`text_shadow.rs`), else None (drawn offscreen, if it has one).
    pub shadow: Option<BakedShadow>,
}

impl NodeText {
    /// Drops the glyphs its layouts kept for the atlas (a frame draws pieces).
    pub(super) fn forget_glyphs(&mut self) {
        for l in &mut self.layouts {
            l.glyphs = Vec::new();
        }
        match &mut self.set {
            Some(GlyphSet::Counter(c)) => c.units.glyphs = Vec::new(),
            Some(GlyphSet::Scramble(s)) => s.units.glyphs = Vec::new(),
            None => {}
        }
    }
}

impl CTextNode {
    /// Instance node `i` as it is laid out: a text node, or an image node showing an emoji; None for anything else.
    pub(super) fn of(
        inst: &SceneInstance,
        i: usize,
        index: &FontIndex,
        language: &Arc<str>,
    ) -> Option<CTextNode> {
        let cn = inst.cnode(i).and_then(|c| inst.compiled().nodes.get(c))?;
        let (node, p) = (&cn.node, &cn.node.props);
        let emoji = node.kind == NodeType::Image && p.emoji.is_some() && p.asset.is_none();
        if node.kind != NodeType::Text && !emoji {
            return None;
        }
        let b = NodeBindings { inst, node: i };
        let rest = inst.rest(i)?;
        let (font, weight, italic) = match &p.font {
            Some(f) if !emoji => font_ref(f, node, index, &b),
            _ => {
                let (weight, italic) = own_look(node, &b);
                (FontRef::bundled(DEFAULT_FAMILY), weight, italic)
            }
        };
        let size = rest.size as f32;
        let mut looks = vec![TextLook {
            font,
            weight,
            italic,
            size,
        }];
        for s in p.spans.iter().flatten() {
            let w = s.weight.as_ref().and_then(|v| b.number(Field::Weight, v));
            looks.push(TextLook {
                font: looks[0].font.clone(),
                weight: w.map_or(weight, |w| w.round().clamp(1.0, 1000.0) as u16),
                italic: s.italic.as_ref().and_then(|f| b.flag(f)).unwrap_or(italic),
                size: s
                    .size
                    .as_ref()
                    .and_then(|v| b.number(Field::Size, v))
                    .map_or(size, |v| v as f32),
            });
        }
        Some(CTextNode {
            looks,
            letter_spacing: rest.letter_spacing as f32,
            line_height: rest.line_height as f32,
            align: p.align,
            wrap: p
                .wrap
                .as_ref()
                .and_then(|v| inst.length_of(i, Field::Wrap, v)),
            fit: p.fit,
            split: p.split,
            reveal: p.reveal.as_ref().map(|r| r.by.unwrap_or(RevealBy::Chars)),
            language: language.clone(),
            emoji,
        })
    }
}

/// What a scene's text may still lay out: its characters and its texts. Every text a node
/// can show (its own, each hold, text key and tween text, each distinct one once) is charged, the sum, not the
/// longest; past the room it isn't laid out, and the scene says `frame.sceneTextCut`.
pub(super) struct Room {
    pub chars: usize,
    pub texts: usize,
    /// The em's largest size in target pixels (the target's longest side, at most `MAX_EM_PX`).
    pub max_px: f32,
}

/// The most distinct texts one scene lays out.
pub(super) const MAX_TEXTS: usize = 256;
/// A text a node shows that wasn't laid out (past the room): it draws nothing.
pub(super) const NOT_LAID: u16 = u16::MAX;

/// Instance node `i` laid out in a box of `box_px` pixels (`px` a 1080p pixel), charging what it lays out to `room`.
/// None for a node that isn't text.
#[allow(clippy::too_many_arguments)]
pub(super) fn node_text(
    fonts: &mut SharedFonts,
    index: &FontIndex,
    inst: &SceneInstance,
    i: usize,
    box_px: (f32, f32),
    px: f32,
    language: &Arc<str>,
    room: &mut Room,
    notes: &mut TextNotes,
) -> Option<NodeText> {
    let node = CTextNode::of(inst, i, index, language)?;
    let paint = TextPaint::of(inst, i);
    let mut left = room.chars.min(Options::NODE.room);
    let max_px = room_px(room);
    let base = |left: usize, grain| Options {
        room: left,
        grain,
        max_px,
        ..Options::NODE
    };
    let mut out = NodeText {
        layouts: Vec::new(),
        layout_of: Vec::new(),
        set: None,
        cells: Vec::new(),
        paint,
        most: 0,
        units: 0,
        reveal: node.reveal.is_some(),
        box_px,
        look_boxes: Vec::new(),
        shadow: None,
    };
    if let Some(c) = inst.counter(i) {
        let shown = |t: &str| t.chars().filter(|c| !c.is_whitespace()).count();
        // A frame's string: its prefix, a number of at most 24 characters, its suffix.
        let most = shown(c.prefix) + NUMBER_CHARS + shown(c.suffix);
        if most > left || room.texts < 2 {
            notes.cut = true;
            return Some(out);
        }
        let set = CounterSet::build(fonts, index, &node, c, box_px, px, base(left, None));
        charge(most, &mut left, room);
        room.texts -= 2;
        notes.join(set.units.notes);
        out.units = most;
        out.most = most;
        out.set = Some(GlyphSet::Counter(set));
        return Some(out);
    }
    let scramble = inst.scramble_chars(i);
    let grain = scramble.map(|_| Grain::Chars);
    let spans = inst.span_texts(i);
    let emoji = inst
        .cnode(i)
        .and_then(|c| inst.compiled().nodes.get(c))
        .and_then(|cn| cn.node.props.emoji.clone())
        .filter(|_| node.emoji)
        .map(|e| inst.text_of(i, &e));
    let own: Vec<(&str, u8)> = match (&emoji, spans.is_empty()) {
        (Some(e), _) => vec![(e.as_str(), 0)],
        (None, true) => vec![(inst.texts(i).first().map_or("", String::as_str), 0)],
        (None, false) => spans
            .iter()
            .enumerate()
            .map(|(k, s)| (s.as_str(), (k + 1).min(255) as u8))
            .collect(),
    };
    let texts = inst.texts(i);
    let shown = (0..texts.len().max(1)).map(|k| match k {
        0 => own.clone(),
        k => vec![(texts[k].as_str(), 0)],
    });
    // Each distinct text once (a scene repeats `$title`), in order, while the room lasts.
    let mut laid_as: Vec<(&str, u16)> = Vec::new();
    for (k, parts) in shown.enumerate() {
        let key = texts.get(k).map_or("", String::as_str);
        if let Some(&(_, at)) = laid_as.iter().find(|(t, _)| *t == key) {
            out.layout_of.push(at);
            continue;
        }
        if room.texts == 0 || (left == 0 && !key.is_empty()) {
            notes.cut = true;
            out.layout_of.push(NOT_LAID);
            continue;
        }
        let laid =
            TextUnits::lay_out_parts(fonts, index, &node, &parts, box_px, px, base(left, grain));
        charge(laid.text.chars().count(), &mut left, room);
        room.texts -= 1;
        let at = out.layouts.len() as u16;
        laid_as.push((key, at));
        out.layout_of.push(at);
        out.layouts.push(laid);
    }
    if let Some(letters) = scramble {
        let n = letters.iter().filter(|c| !c.is_whitespace()).count();
        if n <= left && room.texts > 0 {
            let scale = out.layouts.first().map_or(1.0, |l| l.scale);
            let set = ScrambleSet::build(
                fonts,
                index,
                &node,
                letters,
                scale,
                box_px,
                px,
                base(left, None),
            );
            charge(n, &mut left, room);
            room.texts -= 1;
            notes.join(set.units.notes);
            out.set = Some(GlyphSet::Scramble(set));
        } else {
            notes.cut = true;
        }
    }
    if let Some(l) = out.layouts.first() {
        for p in &l.pieces {
            let k = usize::from(p.look);
            if out.look_boxes.len() <= k {
                out.look_boxes
                    .resize(k + 1, [f32::MAX, f32::MAX, f32::MIN, f32::MIN]);
            }
            let b = &mut out.look_boxes[k];
            *b = [
                b[0].min(p.rect[0]),
                b[1].min(p.rect[1]),
                b[2].max(p.rect[2]),
                b[3].max(p.rect[3]),
            ];
        }
    }
    for l in &out.layouts {
        notes.join(l.notes);
        out.units = out.units.max(l.units);
        let backgrounds = out.paint.backgrounds.iter().flatten().count() * l.lines.len().max(1);
        out.most = out.most.max(l.pieces.len() + backgrounds);
    }
    out.shadow = super::text_shadow::baked(inst, i, &out).map(|sigma| BakedShadow {
        sigma,
        cells: Vec::new(),
    });
    Some(out)
}

/// `n` characters laid out: off the node's `left` and the scene's room.
fn charge(n: usize, left: &mut usize, room: &mut Room) {
    *left = left.saturating_sub(n);
    room.chars = room.chars.saturating_sub(n);
}

/// The em's largest size in target pixels for `room`.
fn room_px(room: &Room) -> f32 {
    room.max_px.clamp(1.0, super::text::MAX_EM_PX)
}
