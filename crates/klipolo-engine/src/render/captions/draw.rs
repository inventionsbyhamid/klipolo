//! The caption renderer's draw half: a frame's captions as instanced quads over the page's atlas
//! (`captions.wgsl`'s `quad` and `shade`), written in four groups in one buffer and drawn with one instanced draw, which
//! keeps their order: every visible word's shadow, then its outline, then the boxes (the line boxes' smooth union, the
//! box behind the word being said, a highlighted word's box), then every fill with its glow. Each word's quad is its
//! atlas cell placed at `block centre + R(turn) × (block scale × (word centre + offset × em))`, sized
//! `cell × block scale × word scale`.
//!
//! Per frame: at most `3 × 32 + 2 + 32` instances of 96 bytes written into a kept `Vec` (no allocation once warm) and
//! uploaded in one write; 96 bytes of uniform (the output size and the line boxes).

use klipolo_model::motion::Ease;

pub(super) use super::draw_pipe::{Buffers, DrawPipe, Instance, View};
use super::layout::PageLayout;
use crate::plan::CaptionLayer;
use crate::plan::captions::frame::MAX_WORDS;
use crate::plan::captions::place::edge_em;

/// The shader's kinds.
const SHADOW: f32 = 0.0;
const OUTLINE: f32 = 1.0;
const BOX: f32 = 2.0;
const FILL: f32 = 3.0;
/// The most line boxes the shader unites (autofit's extra line included).
pub(super) const MAX_LINE_BOXES: usize = 5;
/// A highlighted word's box when the style has no box of its own (Highlight's): padding and radius in em.
const HIGHLIGHT_BOX: (f32, f32, f32) = (0.16, 0.08, 0.18);

fn unit(rgb: [u8; 3], a: f32) -> [f32; 4] {
    let c = |x: u8| f32::from(x) / 255.0;
    [c(rgb[0]), c(rgb[1]), c(rgb[2]), a]
}

/// A box's rect (centre x, centre y, width, height px about the block's centre).
type Rect = [f32; 4];

fn lerp(a: Rect, b: Rect, k: f32) -> Rect {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * k)
}

/// `layer`'s quads for its page laid out as `layout` (atlas `atlas` px) in an output of `out` px, into `into` (cleared
/// first, in the four groups' order), and the frame's uniform into `view`. Nothing is allocated once `into` has room.
pub(super) fn instances(
    layer: &CaptionLayer,
    layout: &PageLayout,
    atlas: (u32, u32),
    out: (u32, u32),
    into: &mut Vec<Instance>,
    view: &mut View,
) {
    into.clear();
    *view = View {
        size: [out.0 as f32, out.1 as f32],
        ..View::default()
    };
    let (f, style, colors) = (&layer.frame, &*layer.style, &layer.colors);
    let Some(pages) = layer.pages() else {
        return;
    };
    let Some(page) = pages.pages.get(layer.page as usize) else {
        return;
    };
    let words = pages
        .words
        .get(page.words.0 as usize..page.words.1 as usize)
        .unwrap_or_default();
    if layout.words.is_empty() || layout.lines.is_empty() {
        return;
    }
    // Which laid-out word each page word is (`PageLayout::words` holds the ones that aren't hidden).
    let mut slot = [u8::MAX; MAX_WORDS];
    let mut next = 0u8;
    for (i, w) in words.iter().enumerate().take(MAX_WORDS) {
        if !w.hidden {
            slot[i] = next;
            next += 1;
        }
    }
    let em = layout.font_px * layout.autofit;
    // The block: its lines' extent (their box padding included), its outline or box padding at its largest word.
    let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for line in &layout.lines {
        (l, t, r, b) = (
            l.min(line[0]),
            t.min(line[1]),
            r.max(line[2]),
            b.max(line[3]),
        );
    }
    let biggest = layout.words.iter().map(|w| w.scale).fold(1.0, f32::max);
    let edge = edge_em(style) as f32 * em * biggest;
    let (w_px, h_px) = (out.0 as f32, out.1 as f32);
    let block = (
        f64::from((r - l + 2.0 * edge) / w_px),
        f64::from((b - t) / h_px),
    );
    let (cx, cy) = layer.anchor.centre(style, layer.format, block);
    let centre = [cx as f32 * w_px, cy as f32 * h_px];
    let turn = f.rotation_deg.to_radians();
    let (sin, cos) = turn.sin_cos();
    let s = f.scale;
    // A point of the block (px about its centre, before the block's scale) on the output.
    let at = |p: [f32; 2]| {
        let (x, y) = (p[0] * s, p[1] * s);
        [centre[0] + cos * x - sin * y, centre[1] + sin * x + cos * y]
    };
    let (aw, ah) = (atlas.0.max(1) as f32, atlas.1.max(1) as f32);
    let offset = [f.dx_em * em, f.dy_em * em];
    // The visible words, each with its laid-out box.
    let visible = || {
        f.words
            .iter()
            .enumerate()
            .filter(|(_, d)| d.visible && d.opacity > 0.0)
            .filter_map(|(i, d)| Some((d, layout.words.get(usize::from(*slot.get(i)?))?)))
    };
    let quad = |d: &crate::plan::captions::frame::WordDraw,
                w: &super::layout::WordBox,
                shift: [f32; 2],
                kind: f32,
                color: [f32; 4],
                color2: [f32; 4]| {
        let [x, y, cw, ch] = w.cell;
        let k = s * d.scale;
        let p = at([
            w.centre[0] + offset[0] + shift[0] * d.scale,
            w.centre[1] + offset[1] + d.dy_em * em + shift[1] * d.scale,
        ]);
        Instance {
            rect: [p[0], p[1], cw as f32 * k, ch as f32 * k],
            uv: [
                x as f32 / aw,
                y as f32 / ah,
                (x + cw) as f32 / aw,
                (y + ch) as f32 / ah,
            ],
            color,
            color2,
            params: [turn, kind, f.opacity * d.opacity, 0.0],
            extra: [0.0; 4],
        }
    };
    // 1. Shadows.
    if let Some(sh) = &style.shadow {
        let shift = [sh.dx_em as f32 * em, sh.dy_em as f32 * em];
        let color = unit(colors.shadow, sh.alpha as f32);
        into.extend(visible().map(|(d, w)| quad(d, w, shift, SHADOW, color, [0.0; 4])));
    }
    // 2. Outlines.
    if style.outline.is_some() {
        let color = unit(colors.outline, 1.0);
        into.extend(visible().map(|(d, w)| quad(d, w, [0.0; 2], OUTLINE, color, [0.0; 4])));
    }
    // 3. Boxes: the line boxes as one shape, the box behind the word being said, the highlighted words'.
    let (pad, radius) = style
        .boxes
        .as_ref()
        .map_or(([HIGHLIGHT_BOX.0, HIGHLIGHT_BOX.1], HIGHLIGHT_BOX.2), |b| {
            ([b.pad_x_em as f32, b.pad_y_em as f32], b.radius_em as f32)
        });
    let radius = radius * em * s;
    let boxed = |rect: Rect, dy_em: f32, color: [f32; 4], alpha: f32| {
        let p = at([rect[0] + offset[0], rect[1] + offset[1] + dy_em * em]);
        let (w, h) = (rect[2] * s, rect[3] * s);
        Instance {
            rect: [p[0], p[1], w + 4.0, h + 4.0],
            color,
            color2: [0.0, 0.0, w, h],
            params: [turn, BOX, alpha, 0.0],
            extra: [0.0, radius, 0.0, 0.0],
            ..Instance::default()
        }
    };
    if let Some((rgb, alpha)) = f.line_boxes {
        let mid = [(l + r) / 2.0, (t + b) / 2.0];
        for (k, line) in layout.lines.iter().take(MAX_LINE_BOXES).enumerate() {
            view.lines[k] = [
                ((line[0] + line[2]) / 2.0 - mid[0]) * s,
                ((line[1] + line[3]) / 2.0 - mid[1]) * s,
                (line[2] - line[0]) * s,
                (line[3] - line[1]) * s,
            ];
        }
        let smooth = radius.max(1.0);
        let mut all = boxed(
            [mid[0], mid[1], r - l + smooth, b - t + smooth],
            0.0,
            unit(rgb, alpha),
            f.opacity,
        );
        all.extra = [
            0.0,
            radius,
            smooth,
            layout.lines.len().min(MAX_LINE_BOXES) as f32,
        ];
        into.push(all);
    }
    let around = |w: &super::layout::WordBox, scale: f32| -> Rect {
        [
            w.centre[0],
            w.centre[1],
            (w.size[0] + 2.0 * pad[0] * em) * scale,
            (w.size[1] + 2.0 * pad[1] * em) * scale,
        ]
    };
    if let Some(wb) = f.word_box
        && let Some(&k) = slot.get(usize::from(wb.to)).filter(|&&k| k != u8::MAX)
        && let (Some(to), Some(d)) = (
            layout.words.get(usize::from(k)),
            f.words.get(usize::from(wb.to)),
        )
    {
        let target = around(to, d.scale);
        let from = wb
            .from
            .and_then(|i| slot.get(usize::from(i)).filter(|&&k| k != u8::MAX))
            .and_then(|&k| layout.words.get(usize::from(k)))
            .filter(|from| from.line == to.line);
        let rect = match from {
            // On one line: it glides from the word before.
            Some(from) => lerp(
                around(from, 1.0),
                target,
                Ease::Power2Out.apply(f64::from(wb.glide)) as f32,
            ),
            // A line's first word: it grows in from its middle.
            None => {
                let grow = Ease::Power2Out.apply(f64::from(wb.grow)) as f32;
                [target[0], target[1], target[2] * grow, target[3]]
            }
        };
        into.push(boxed(
            rect,
            d.dy_em,
            unit(wb.color, wb.alpha),
            f.opacity * d.opacity,
        ));
    }
    for (d, w) in visible() {
        if let Some(rgb) = d.highlight {
            into.push(boxed(
                around(w, d.scale),
                d.dy_em,
                unit(rgb, 1.0),
                f.opacity * d.opacity,
            ));
        }
    }
    // 4. Fills, each with its glow.
    into.extend(visible().map(|(d, w)| {
        let glow = d.glow.map_or([0.0; 4], |(rgb, a, _)| unit(rgb, a));
        quad(d, w, [0.0; 2], FILL, unit(d.fill, d.fill_alpha), glow)
    }));
}
