//! A scene instance's text atlas: every piece its texts and glyph sets draw, drawn once into its own cell
//! when the instance is prepared, never per frame. A cell holds two masks, as the caption atlas packs them: R the
//! fill's coverage, G the silhouette (the fill with its outline). Glyphs in colour (emoji) go to a colour plane only as
//! large as the texels they cover, at the same places, only when there are any; a piece of one glyph
//! that another drew before it (the same image: its subpixel bin and its outline) shares that one's cell. Glyphs
//! are rasterised with cosmic-text's swash cache (the rasteriser the vendored `GlyphLayer` draws with) at their places
//! in the whole text's layout, so kerning, ligatures and joined scripts keep their shaped forms in every piece; an
//! outline is the glyph's own stroke (swash), round-joined.
//!
//! Bounded: an atlas holds at most `budget` bytes (its format's share, `text_cache.rs`). Its scale comes from a tight
//! estimate of its cells (each piece's advance by about its ink's height, with the outline and a pixel), then from the
//! size the drawn cells really took: drawn sharp when that fits, else softer and magnified (said once as
//! `engine.frame.textsSofter`), and refused outright (None, said as `frame.sceneTextCut`) when even `MIN_SCALE` doesn't
//! fit (the atlas cap). Nothing is ever kept past the budget, and no cell is wider than a side.

use std::collections::HashMap;

use glyphon::FontSystem;
use glyphon::cosmic_text::{CacheKey, SwashCache, SwashContent, SwashImage};
use swash::scale::ScaleContext;

use super::text::TextUnits;
use super::text_blit::{Channel, Ready, blit, glyphs_of, grow, stroke_of};
use super::text_shadow::ShadowImage;

/// Cells are packed into rows no wider than this.
const ROW_PX: u32 = 2048;
/// An atlas side never exceeds this.
const MAX_SIDE: u32 = 8192;
/// The softest an atlas is drawn before it is refused: a sixteenth of its pixels each way.
const MIN_SCALE: f32 = 1.0 / 16.0;
/// How many times an atlas is drawn at most (the estimate, a correction or two, a sharper retry).
const MAX_DRAWS: usize = 5;

/// Where one piece is drawn from: its cell's box in its text's layout (node box pixels) and in the atlas (0–1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Cell {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
}

/// The colour glyphs' premultiplied pixels (four bytes a texel), over only the atlas texels they cover: that region's
/// top left in the atlas and its size.
pub(super) struct ColorPlane {
    pub at: [u32; 2],
    pub size: (u32, u32),
    pub pixels: Vec<u8>,
}

/// An atlas's pixels: the masks (two bytes a pixel), the colour glyphs' plane (when a glyph has colour), its cells in
/// the order the texts' pieces come, the scale it was drawn at, and its baked shadows (`text_shadow.rs`).
pub(super) struct AtlasImage {
    pub size: (u32, u32),
    pub masks: Vec<u8>,
    pub colors: Option<ColorPlane>,
    pub cells: Vec<Cell>,
    pub scale: f32,
    pub shadows: Option<ShadowImage>,
}

impl AtlasImage {
    /// The GPU bytes it takes once uploaded.
    pub(super) fn bytes(&self) -> u64 {
        let px = u64::from(self.size.0) * u64::from(self.size.1);
        let colors = self
            .colors
            .as_ref()
            .map_or(0, |c| u64::from(c.size.0) * u64::from(c.size.1) * 4);
        let shadows = self
            .shadows
            .as_ref()
            .map_or(0, |s| u64::from(s.size.0) * u64::from(s.size.1));
        px * 2 + colors + shadows
    }
}

/// The atlas of `texts` (each with its outline's width in layout pixels), within `budget` bytes; None when even its
/// softest is past it.
pub(super) fn rasterize(
    fs: &mut FontSystem,
    texts: &[(&TextUnits, f32)],
    budget: u64,
) -> Option<AtlasImage> {
    let mut scale = estimate_scale(texts, budget);
    for _ in 0..MAX_DRAWS {
        if scale < MIN_SCALE {
            return None;
        }
        match draw(fs, texts, scale, budget) {
            Ok(image) => {
                // The estimate was careful: once, as sharp as the cells really drawn allow.
                let room = (budget as f64 / image.bytes().max(1) as f64).sqrt() as f32;
                let sharper = (scale * room * 0.97).min(1.0);
                if scale < 1.0
                    && sharper > scale * 1.1
                    && let Ok(better) = draw(fs, texts, sharper, budget)
                {
                    return Some(better);
                }
                return Some(image);
            }
            Err(shrink) => scale *= shrink.min(0.97),
        }
    }
    None
}

/// The largest scale (1 at most) whose cells fit `budget`, each its piece's advance by its line or 1.3 em (whichever
/// is less: about its ink) with the outline and a pixel round it; and none wider than an atlas side.
fn estimate_scale(texts: &[(&TextUnits, f32)], budget: u64) -> f32 {
    let (mut area, mut widest) = (0.0f64, 0.0f64);
    for (t, outline) in texts {
        let pad = f64::from(*outline) + 2.0;
        let em = f64::from(t.font_px).max(1.0);
        for p in &t.pieces {
            let w = f64::from(p.rect[2] - p.rect[0]) + 2.0 * pad;
            let h = f64::from(p.rect[3] - p.rect[1]).min(1.3 * em) + 2.0 * pad;
            area += w.max(1.0) * h.max(1.0);
            widest = widest.max(w);
        }
    }
    let colour = texts.iter().any(|(t, _)| t.text.chars().any(maybe_colour));
    let bytes = area * if colour { 6.0 } else { 2.0 };
    let fits = if bytes <= budget as f64 {
        1.0
    } else {
        (budget as f64 / bytes).sqrt()
    };
    fits.min(f64::from(MAX_SIDE - 2) / widest.max(1.0)).min(1.0) as f32
}

/// A character that may be drawn in colour (an emoji's), for the estimate only.
fn maybe_colour(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0xFE0F | 0x200D)
}

/// Every piece's glyphs and outline rasterised at `scale` into cells packed in rows; Err with a factor to shrink the
/// scale by when they would pack past an atlas side, or hold more than `budget` bytes: checked before its pixels are
/// made, so an atlas past the budget is never allocated.
pub(super) fn draw(
    fs: &mut FontSystem,
    texts: &[(&TextUnits, f32)],
    scale: f32,
    budget: u64,
) -> Result<AtlasImage, f32> {
    let mut cache = SwashCache::new();
    let mut strokes: Vec<SwashImage> = Vec::new();
    let mut context = ScaleContext::new();
    let mut stroked: HashMap<(CacheKey, u32), usize> = HashMap::new();
    // Each piece's glyphs placed, and its ink's box in whole pixels at the scale.
    let mut placed: Vec<(Vec<Ready>, [i32; 4])> = Vec::new();
    for (t, outline) in texts {
        let width = (outline * scale).max(0.0);
        for p in &t.pieces {
            let mut ready = Vec::new();
            let mut ink = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
            for g in glyphs_of(t, p) {
                let phys = g.physical((0.0, 0.0), scale);
                let (x, y) = (phys.x, (p.baseline * scale).round() as i32 + phys.y);
                if let Some(img) = cache.get_image(fs, phys.cache_key) {
                    grow(&mut ink, x + img.placement.left, y - img.placement.top, img);
                }
                let stroke = (width > 0.0)
                    .then(|| {
                        let mut s = (&mut context, &mut strokes, &mut stroked);
                        stroke_of(fs, &mut s, phys.cache_key, width)
                    })
                    .flatten()
                    .map(|k| {
                        let s = &strokes[k];
                        grow(&mut ink, x + s.placement.left, y - s.placement.top, s);
                        (x + s.placement.left, y - s.placement.top, k)
                    });
                ready.push(Ready {
                    key: phys.cache_key,
                    x,
                    y,
                    stroke,
                });
            }
            if ink[0] > ink[2] {
                // Nothing to draw (a blank glyph): an empty cell at the piece's start.
                let (x, y) = ((p.rect[0] * scale) as i32, (p.rect[1] * scale) as i32);
                ink = [x, y, x, y];
            }
            placed.push((ready, ink));
        }
    }
    // Cells: each ink box with a pixel's gutter, packed in rows; a piece of one glyph drawn before (the same image and
    // outline: its key holds its subpixel bin) takes that one's cell.
    let (mut x, mut y, mut row_h, mut width) = (0u32, 0u32, 0u32, 1u32);
    let mut origins = Vec::with_capacity(placed.len());
    let mut first: HashMap<(CacheKey, Option<usize>), usize> = HashMap::new();
    for (k, (ready, ink)) in placed.iter().enumerate() {
        let w = (ink[2] - ink[0]).max(0) as u32 + 2;
        let h = (ink[3] - ink[1]).max(0) as u32 + 2;
        if let [one] = &ready[..] {
            let key = (one.key, one.stroke.map(|s| s.2));
            if let Some(&j) = first.get(&key) {
                let (cx, cy, _, _, _) = origins[j];
                origins.push((cx, cy, w, h, false));
                continue;
            }
            first.insert(key, k);
        }
        if x > 0 && x + w > ROW_PX {
            y += row_h;
            (x, row_h) = (0, 0);
        }
        origins.push((x, y, w, h, true));
        x += w;
        row_h = row_h.max(h);
        width = width.max(x);
    }
    let (w, h) = (width.max(1), (y + row_h).max(1));
    if w > MAX_SIDE {
        return Err(MAX_SIDE as f32 / w as f32);
    }
    if h > MAX_SIDE {
        return Err((MAX_SIDE as f32 / h as f32).sqrt());
    }
    let size = (w, h);
    let px = size.0 as usize * size.1 as usize;
    // The colour glyphs' texels, where they land in the atlas: the colour plane covers only these.
    let mut colour = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for ((ready, ink), &(cx, cy, _, _, own)) in placed.iter().zip(&origins) {
        let (dx, dy) = (cx as i32 + 1 - ink[0], cy as i32 + 1 - ink[1]);
        for g in ready.iter().filter(|_| own) {
            if let Some(img) = cache.get_image(fs, g.key)
                && img.content == SwashContent::Color
            {
                let (gx, gy) = (g.x + img.placement.left + dx, g.y - img.placement.top + dy);
                let (gw, gh) = (img.placement.width as i64, img.placement.height as i64);
                colour = [
                    colour[0].min(i64::from(gx)),
                    colour[1].min(i64::from(gy)),
                    colour[2].max(i64::from(gx) + gw),
                    colour[3].max(i64::from(gy) + gh),
                ];
            }
        }
    }
    let plane = (colour[0] < colour[2]).then(|| {
        let lo = [colour[0].max(0) as u32, colour[1].max(0) as u32];
        let hi = [
            (colour[2].max(0) as u32).min(size.0),
            (colour[3].max(0) as u32).min(size.1),
        ];
        (lo, ((hi[0] - lo[0]).max(1), (hi[1] - lo[1]).max(1)))
    });
    let colour_px = plane.map_or(0, |(_, s)| u64::from(s.0) * u64::from(s.1));
    let bytes = px as u64 * 2 + colour_px * 4;
    if bytes > budget {
        return Err((budget as f64 / bytes as f64).sqrt() as f32);
    }
    let mut out = AtlasImage {
        size,
        masks: vec![0; px * 2],
        colors: plane.map(|(at, size)| ColorPlane {
            at,
            size,
            pixels: vec![0; size.0 as usize * size.1 as usize * 4],
        }),
        cells: Vec::with_capacity(placed.len()),
        scale,
        shadows: None,
    };
    for ((ready, ink), &(cx, cy, w, h, own)) in placed.iter().zip(&origins) {
        // A glyph at (x, y) in the layout lands at (x + dx, y + dy) in the atlas: a whole-pixel move, so the cached
        // image (its subpixel bin) is the one to draw.
        let (dx, dy) = (cx as i32 + 1 - ink[0], cy as i32 + 1 - ink[1]);
        for g in ready.iter().filter(|_| own) {
            if let Some((sx, sy, k)) = g.stroke {
                blit(&mut out, &strokes[k], sx + dx, sy + dy, Channel::Silhouette);
            }
            if let Some(img) = cache.get_image(fs, g.key) {
                let (gx, gy) = (g.x + img.placement.left + dx, g.y - img.placement.top + dy);
                blit(&mut out, img, gx, gy, Channel::Fill);
            }
        }
        let (sw, sh) = (size.0 as f32, size.1 as f32);
        let left = (ink[0] - 1) as f32 / scale;
        let top = (ink[1] - 1) as f32 / scale;
        out.cells.push(Cell {
            rect: [left, top, left + w as f32 / scale, top + h as f32 / scale],
            uv: [
                cx as f32 / sw,
                cy as f32 / sh,
                (cx + w) as f32 / sw,
                (cy + h) as f32 / sh,
            ],
        });
    }
    Ok(out)
}
