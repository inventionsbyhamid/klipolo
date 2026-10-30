//! Text shadows baked once: a text node whose shadow's blur never changes and which draws one piece
//! at a time (an unsplit line in one look, no span background, nothing clipped: `baked`) has its shadow drawn in the
//! text pass from a cell of its own, the piece's silhouette (the atlas's G: the fill with its outline, colour glyphs
//! included) blurred on the text build thread, as the caption atlas carries its shadow. A fitted text's box clips its
//! letters, not its shadow, as offscreen; so only a text its box doesn't cut bakes its shadow. Its colour, offset and opacity
//! may move; it is drawn under the piece in the piece's own quad and faded with it, as CSS fades an element with its
//! shadow, so it is the offscreen chain's picture without its five passes and its target. Any other text
//! shadow keeps that chain: pieces that move apart, overlap or clip cast one shadow of their whole silhouette there.
//!
//! The blur is the built-in blur's (`registry/wgsl/blur.wgsl`): a Gaussian of half the shadow's blur, its taps out to
//! the blur in pairs, weighed as transparent past the silhouette, drawn small past six texels of deviation
//! (`text_shadow_blur.rs`: bounded whatever the blur). Cells are packed in rows into one R8 image, bounded
//! by the atlas's own budget and the device's texture side (past either, those shadows are cast offscreen); the
//! shadow's ink never reaches past its cell.

use std::collections::HashMap;

use klipolo_model::scene::instance::SceneInstance;
use klipolo_model::scene::timeline::Prop;

use super::text_atlas::{AtlasImage, Cell};
use super::text_nodes::NodeText;
use super::text_shadow_blur::{Small, blur_small};

/// Shadow cells are packed into rows no wider than this.
const ROW_PX: u32 = 2048;

/// The shadow's Gaussian, in 1080p pixels (half its blur, as CSS's), when node `i`'s shadow may be drawn from a baked
/// cell: its blur, the node's blur and its size never move, and every text it shows is one piece, with no glyph set and
/// no span background (its box mustn't cut it either: `fits`, once drawn); None otherwise, or when it never shows one.
pub(super) fn baked(inst: &SceneInstance, i: usize, nt: &NodeText) -> Option<f32> {
    let rest = inst.rest(i)?;
    let moves = |which: &[Prop]| inst.animates(i, |q| which.contains(&q));
    let shows = rest.shadow.color[3] > 0.0 || moves(&[Prop::ShadowA]);
    let still = !moves(&[Prop::ShadowBlur, Prop::Blur, Prop::Size]) && rest.blur <= 0.0;
    let one = nt.set.is_none()
        && nt.layouts.iter().all(|l| l.pieces.len() <= 1)
        && nt.paint.backgrounds.iter().all(Option::is_none);
    (shows && still && one).then_some((rest.shadow.blur.max(0.0) / 2.0) as f32)
}

/// Whether a piece drawn from `cell` of a text kept to its node's box of `box_px` (a fitted text's `clip`) is whole
/// inside it: the box cuts nothing of its silhouette, so its shadow is the same baked as cast offscreen.
pub(super) fn fits(cell: &Cell, (w, h): (f32, f32), clip: bool) -> bool {
    let r = cell.rect;
    !clip || (r[0] >= -1.5 && r[1] >= -1.5 && r[2] <= w + 1.5 && r[3] <= h + 1.5)
}

/// A text node's baked shadow: its Gaussian in 1080p pixels, and each of its layouts' shadow cell (an index into
/// `TextLayouts::shadows`; `u32::MAX` for a layout with nothing to draw).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BakedShadow {
    pub sigma: f32,
    pub cells: Vec<u32>,
}

/// The baked shadows' pixels: one byte a texel, the silhouette blurred.
pub(super) struct ShadowImage {
    pub size: (u32, u32),
    pub pixels: Vec<u8>,
    /// The taps and texel reads its bake took (the bound's test).
    #[cfg_attr(not(test), allow(dead_code))]
    pub work: u64,
}

/// The shadow cells of the atlas `atlas`'s cells `which` (each with its Gaussian in atlas texels), blurred (small past
/// six texels: `text_shadow_blur.rs`), in the order asked; a cell asked twice with the same Gaussian is drawn once
/// None when nothing is asked, or when they would take more than `budget` bytes or a side past `max_side` (the
/// device's).
pub(super) fn bake(
    atlas: &AtlasImage,
    which: &[(usize, f32)],
    budget: u64,
    max_side: u32,
) -> Option<(ShadowImage, Vec<Cell>)> {
    if which.is_empty() {
        return None;
    }
    let (aw, ah) = (atlas.size.0 as f32, atlas.size.1 as f32);
    // Each distinct (cell, Gaussian) once: its source texels, how it is blurred, and where it goes.
    let mut drawn: HashMap<(usize, u32), usize> = HashMap::new();
    let mut jobs: Vec<([u32; 4], Small, [u32; 2])> = Vec::new();
    let mut of: Vec<usize> = Vec::with_capacity(which.len());
    let (mut x, mut y, mut row_h, mut width) = (0u32, 0u32, 0u32, 1u32);
    for &(cell, sigma) in which {
        let c = atlas.cells.get(cell)?;
        let key = (cell, sigma.to_bits());
        if let Some(&k) = drawn.get(&key) {
            of.push(k);
            continue;
        }
        let src = [
            (c.uv[0] * aw).round() as u32,
            (c.uv[1] * ah).round() as u32,
            (c.uv[2] * aw).round() as u32,
            (c.uv[3] * ah).round() as u32,
        ];
        let small = Small::of(sigma);
        let (w, h) = small.size((src[2] - src[0], src[3] - src[1]));
        if x > 0 && x + w > ROW_PX {
            y += row_h;
            (x, row_h) = (0, 0);
        }
        drawn.insert(key, jobs.len());
        of.push(jobs.len());
        jobs.push((src, small, [x, y]));
        x += w;
        row_h = row_h.max(h);
        width = width.max(x);
    }
    let size = (width.max(1), (y + row_h).max(1));
    if u64::from(size.0) * u64::from(size.1) > budget || size.0.max(size.1) > max_side {
        return None;
    }
    let mut pixels = vec![0u8; size.0 as usize * size.1 as usize];
    let mut cells = Vec::with_capacity(jobs.len());
    let scale = atlas.scale.max(1e-3);
    let mut work = 0u64;
    for (src, small, at) in &jobs {
        let (w, h) = (src[2] - src[0], src[3] - src[1]);
        let g = |x: u32, y: u32| {
            let at = (src[1] + y) as usize * atlas.size.0 as usize + (src[0] + x) as usize;
            f32::from(atlas.masks[at * 2 + 1]) / 255.0
        };
        work += blur_small(g, (w, h), small, (&mut pixels, size.0), *at);
        // Its box in the layout: the piece's cell grown by the margin, at the step's texels.
        let (s, m) = (small.step as f32, small.margin() as f32);
        let (iw, ih) = (w.div_ceil(small.step) as f32, h.div_ceil(small.step) as f32);
        let (cw, ch) = small.size((w, h));
        cells.push((
            *at,
            (cw, ch),
            [m * s / scale, (iw + m) * s / scale, (ih + m) * s / scale],
        ));
    }
    let (sw, sh) = (size.0 as f32, size.1 as f32);
    // Each asked cell's shadow cell: where its piece's cell rests, grown by the margin; its texels in the image.
    let out = which
        .iter()
        .zip(&of)
        .map(|(&(cell, _), &k)| {
            let c = atlas.cells[cell];
            let ([x, y], (w, h), [before, across, down]) = cells[k];
            Cell {
                rect: [
                    c.rect[0] - before,
                    c.rect[1] - before,
                    c.rect[0] + across,
                    c.rect[1] + down,
                ],
                uv: [
                    x as f32 / sw,
                    y as f32 / sh,
                    (x + w) as f32 / sw,
                    (y + h) as f32 / sh,
                ],
            }
        })
        .collect();
    Some((ShadowImage { size, pixels, work }, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An atlas of one `w` × `h` cell, its silhouette solid but for a texel's gutter.
    fn solid(w: u32, h: u32) -> AtlasImage {
        let mut masks = vec![0u8; (w * h * 2) as usize];
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                masks[((y * w + x) * 2 + 1) as usize] = 255;
            }
        }
        AtlasImage {
            size: (w, h),
            masks,
            colors: None,
            cells: vec![Cell {
                rect: [0.0, 0.0, w as f32, h as f32],
                uv: [0.0, 0.0, 1.0, 1.0],
            }],
            scale: 1.0,
            shadows: None,
        }
    }

    /// A shadow's bake is bounded whatever its blur, up to the largest (a blur of 256 is σ = 128 1080p
    /// pixels: 256 atlas texels at 4K), about what a small one costs: drawn small, as the letters' blur is. At full
    /// resolution a 400 × 100 cell took about 77 million taps at σ = 64 and 1.7 billion at 256.
    #[test]
    fn a_shadows_bake_is_bounded_at_any_blur() {
        let (w, h) = (400u64, 100u64);
        let bound = 60 * (w + 64) * (h + 64);
        let atlas = solid(w as u32, h as u32);
        for sigma in [2.0f32, 5.9, 64.0, 256.0] {
            let (image, _) = bake(&atlas, &[(0, sigma)], u64::MAX, 8192).unwrap();
            assert!(
                image.work <= bound,
                "σ {sigma}: {} taps against {bound}",
                image.work
            );
        }
    }

    /// A shadow image past the device's texture side isn't made (that shadow is cast offscreen).
    #[test]
    fn a_shadow_past_the_devices_side_isnt_baked() {
        let atlas = solid(4100, 40);
        assert!(bake(&atlas, &[(0, 2.0)], u64::MAX, 4096).is_none());
        assert!(bake(&atlas, &[(0, 2.0)], u64::MAX, 8192).is_some());
    }

    /// A one-texel dot blurs into its cell, centred, and nothing of it reaches past the cell's edge; asked twice, it is
    /// drawn once.
    #[test]
    fn a_dot_blurs_within_its_cell() {
        let mut masks = vec![0u8; 16 * 16 * 2];
        masks[(8 * 16 + 8) * 2 + 1] = 255;
        let atlas = AtlasImage {
            size: (16, 16),
            masks,
            colors: None,
            cells: vec![Cell {
                rect: [0.0, 0.0, 16.0, 16.0],
                uv: [0.0, 0.0, 1.0, 1.0],
            }],
            scale: 1.0,
            shadows: None,
        };
        let (image, cells) = bake(&atlas, &[(0, 1.0), (0, 1.0)], 1 << 20, 8192).unwrap();
        let r = Small::of(1.0).margin();
        assert_eq!(image.size, (16 + 2 * r, 16 + 2 * r), "drawn once for both");
        assert_eq!(cells.len(), 2);
        let g = r as f32;
        assert_eq!(cells[0].rect, [-g, -g, 16.0 + g, 16.0 + g]);
        let at = |x: u32, y: u32| image.pixels[(y * image.size.0 + x) as usize];
        let c = 8 + r;
        assert!(
            at(c, c) > at(c + 1, c) && at(c + 1, c) > at(c + 2, c),
            "a peak at the dot"
        );
        assert_eq!(at(c + 4, c), 0, "nothing past the reach");
    }
}
