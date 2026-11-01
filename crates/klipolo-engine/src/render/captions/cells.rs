//! Where a laid-out page's words go: on their lines, centred per line around the block's centre, and in
//! their cells of the page's atlas, each with room for the outline, the shadow and the blurs.

use klipolo_model::captions::style::{BoxOn, CaptionStyleDef};

use super::layout::{PageLayout, WordBox};
use crate::plan::captions::place::word_growth;
use super::shaping::Words;

/// Cells are packed into rows no wider than this.
const ROW_PX: u32 = 2048;
/// Room for ink past a word's advance (italics, script faces, a "j"'s or an "f"'s bearing), in em.
const OVERHANG_EM: f32 = 0.1;
/// An atlas side never exceeds this (wgpu's default texture limit).
const MAX_ATLAS_SIDE: u32 = 8192;

/// Each word's box on its line (lines starting at `starts`, a `space` between two words at scale 1), and each line's
/// rect, at `layout.autofit`. In a style whose words grow as they are said (`word_growth`), two words are as far apart
/// as a space at that growth, plus what each grows into the gap, so a word that grows never touches its neighbour.
pub(super) fn place_words(
    layout: &mut PageLayout,
    style: &CaptionStyleDef,
    starts: &[u8],
    words: &Words,
    space: f32,
) {
    let a = layout.autofit;
    let line_h = layout.font_px * a * style.line_height as f32;
    let block_h = line_h * starts.len() as f32;
    layout.block[1] = block_h;
    let (box_x, box_y) = match &style.boxes {
        Some(b) if b.on == BoxOn::Line => (
            b.pad_x_em as f32 * layout.font_px * a,
            b.pad_y_em as f32 * layout.font_px * a,
        ),
        _ => (0.0, 0.0),
    };
    let grow = word_growth(style) as f32;
    // The gap after word i: a space at the growth, and what i and the word after it grow into it.
    let gap = |i: usize| space * grow + (grow - 1.0) / 2.0 * (words.widths[i] + words.widths[i + 1]);
    let n = words.widths.len();
    for (line, &from) in starts.iter().enumerate() {
        let from = from as usize;
        let to = starts.get(line + 1).map_or(n, |&e| e as usize);
        let gaps: f32 = (from..to.saturating_sub(1)).map(gap).sum();
        let line_w = (words.widths[from..to].iter().sum::<f32>() + gaps) * a;
        let y = -block_h / 2.0 + line_h * (line as f32 + 0.5);
        let mut left = -line_w / 2.0;
        let on_line = words.widths[from..to]
            .iter()
            .zip(&words.heights[from..to])
            .zip(&words.scales[from..to])
            .zip(&words.faces[from..to])
            .zip(&words.ranges[from..to]);
        for (k, ((((&w, &h), &scale), &face), &text)) in on_line.enumerate() {
            let w = w * a;
            layout.words.push(WordBox {
                centre: [left + w / 2.0, y],
                size: [w, h * a],
                cell: [0; 4],
                line: line as u8,
                scale,
                face,
                text,
            });
            if from + k + 1 < to {
                left += w + gap(from + k) * a;
            }
        }
        layout.lines.push([
            -line_w / 2.0 - box_x,
            y - line_h / 2.0 - box_y,
            line_w / 2.0 + box_x,
            y + line_h / 2.0 + box_y,
        ]);
    }
}

/// Room around a word's box at `em` px: its outline, the shadow's offset, the widest blur's reach (3σ, where the
/// blur pass stops), ink past the advance, and 2 px. All in em but the 2 px, so a cell keeps
/// its proportions at every resolution.
pub(super) fn pad(style: &CaptionStyleDef, em: f32) -> f32 {
    let outline = style
        .outline
        .as_ref()
        .map_or(0.0, |o| o.width_em as f32 * em);
    let offset = style
        .shadow
        .as_ref()
        .map_or(0.0, |s| s.dx_em.abs().max(s.dy_em.abs()) as f32 * em);
    let (shadow, glow) = blur_sigmas(style, em);
    outline + offset + 3.0 * shadow.max(glow) + OVERHANG_EM * em + 2.0
}

/// The widest glow of the style (the active word's, the keywords'), px at `em`; 0 without one.
pub(super) fn glow_px(style: &CaptionStyleDef, em: f32) -> f32 {
    let keyword = style.keyword.as_ref().and_then(|k| k.glow.as_ref());
    [style.active.glow.as_ref(), keyword]
        .into_iter()
        .flatten()
        .map(|g| g.blur_em as f32 * em)
        .fold(0.0, f32::max)
}

/// The atlas's blur sigmas at `em` px: the shadow's, and the glow's (−1 when the style has none). Half the style's
/// blur, in em, never capped in pixels, so a shadow or a glow keeps its size to the text in the
/// preview and in every export; the blur pass bounds its cost by spacing its taps instead (`captions.wgsl`). The style
/// file's check keeps a blur within 1 em.
pub fn blur_sigmas(style: &CaptionStyleDef, em: f32) -> (f32, f32) {
    let shadow = style
        .shadow
        .as_ref()
        .map_or(0.0, |s| s.blur_em as f32 * em / 2.0);
    let glow = if glows(style) {
        glow_px(style, em) / 2.0
    } else {
        -1.0
    };
    (shadow, glow)
}

/// Whether the style glows at all (the atlas's A channel).
pub(super) fn glows(style: &CaptionStyleDef) -> bool {
    style.active.glow.is_some() || style.keyword.as_ref().is_some_and(|k| k.glow.is_some())
}

/// The words' cells in reading order, in rows no wider than `ROW_PX` (a wider cell has a row of its own), and the
/// atlas's size.
pub(super) fn pack_cells(layout: &mut PageLayout) {
    let pad = layout.pad;
    let (mut x, mut y, mut row_h, mut width) = (0u32, 0u32, 0u32, 0u32);
    for word in &mut layout.words {
        let w = (word.size[0] + 2.0 * pad).ceil() as u32;
        let h = (word.size[1] + 2.0 * pad).ceil() as u32;
        if x > 0 && x + w > ROW_PX {
            y += row_h;
            x = 0;
            row_h = 0;
        }
        word.cell = [x, y, w, h];
        x += w;
        row_h = row_h.max(h);
        width = width.max(x);
    }
    layout.atlas = (
        width.clamp(1, MAX_ATLAS_SIDE),
        (y + row_h).clamp(1, MAX_ATLAS_SIDE),
    );
}

#[cfg(test)]
mod tests {
    use klipolo_model::captions::styles::CaptionStyles;
    use klipolo_model::contrib::ContribId;

    use super::*;

    /// Word pop's words, each grown as far as a word being said grows, keep a space's width at that size between them.
    #[test]
    fn a_word_that_grows_never_touches_its_neighbour() {
        for key in ["word-pop", "bold-box", "bold", "clean"] {
            let style = CaptionStyles::builtin()
                .get(&ContribId::builtin(&format!("caption.{key}")))
                .unwrap();
            let grow = word_growth(style) as f32;
            let mut words = Words::default();
            for (i, w) in [300.0, 120.0, 420.0].into_iter().enumerate() {
                words.widths.push(w);
                words.heights.push(100.0);
                words.scales.push(1.0);
                words.faces.push(0);
                words.ranges.push([i as u32, i as u32 + 1]);
            }
            let mut layout = PageLayout {
                font_px: 96.0,
                autofit: 1.0,
                ..PageLayout::default()
            };
            let space = 27.0;
            place_words(&mut layout, style, &[0], &words, space);
            for pair in layout.words.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                let gap = (b.centre[0] - b.size[0] * grow / 2.0) - (a.centre[0] + a.size[0] * grow / 2.0);
                assert!(gap >= space * grow - 0.01, "{key}: {gap} between grown words, not {}", space * grow);
            }
            // A style whose words don't grow keeps a plain space.
            if grow == 1.0 {
                let gap = layout.words[1].centre[0] - layout.words[1].size[0] / 2.0
                    - (layout.words[0].centre[0] + layout.words[0].size[0] / 2.0);
                assert!((gap - space).abs() < 0.01, "{key}: {gap}");
            }
        }
    }
}
