//! Outlines as glyphs of their own: a letter's outline `r` px wide is the letter grown
//! by `r` all round, drawn once, where the glyph layer stamped the letter up to 196 times on rings out to `r`. It is
//! rasterised with swash (the rasteriser cosmic-text and glyphon draw glyphs with): the letter's outline stroked `2r`
//! wide, round-joined, over its fill, which is exactly every point within `r` of the letter (as scene text's outlines
//! are drawn). A glyph with no outline to stroke (a colour emoji's bitmap) has its coverage grown by a
//! distance transform instead. The vendored glyph layer draws these as glyphon custom glyphs: one quad a letter, kept
//! in its atlas like any glyph.
//!
//! Each image has a custom-glyph id for as long as the glyph atlas lives (glyphon caches custom glyphs by id and
//! size): ids are never reused until the atlas is made anew (`clear`), which the layer does once `full`. A glyph's image
//! is made when it is first drawn and kept: glyphon asks again whenever its atlas grows (every glyph
//! it holds, each time: a title's first prepare grows it two or three times from 256²) or misses, and a kept image is
//! a copy where a stroke costs a swash render. Kept images stay within `KEEP_BYTES` across prepares (past it, a
//! prepare starts with none kept, and an ask makes its image again from the font kept).

use std::collections::HashMap;
use std::sync::Arc;

use glyphon::cosmic_text::{CacheKey, CacheKeyFlags, Font, fontdb};
use glyphon::{ContentType, RasterizeCustomGlyphRequest, RasterizedCustomGlyph};
use swash::scale::image::{Content, Image};
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Angle, Format, Join, Placement, Stroke, Transform, Vector};

/// Ids made before the atlas is made anew (each a few dozen bytes here).
const MAX_IDS: usize = 16 * 1024;
/// The images kept between prepares, at most (a 4K title's outlines take about 1.5 MB).
const KEEP_BYTES: usize = 2 << 20;

struct Made {
    key: CacheKey,
    width: f32,
    placement: Placement,
}

/// See the module doc.
#[derive(Default)]
pub struct OutlineGlyphs {
    context: ScaleContext,
    /// By custom-glyph id (its index).
    made: Vec<Made>,
    /// By glyph and width: its id, or None for a glyph with nothing to stroke (a space), so it isn't tried again.
    ids: HashMap<(CacheKey, u32), Option<u16>>,
    fonts: HashMap<fontdb::ID, Arc<Font>>,
    /// The images made, by id, handed to glyphon whenever it asks, and their bytes.
    images: HashMap<u16, Vec<u8>>,
    kept: usize,
    /// Outline images made (swash renders), for the cost test and the perf harness.
    rasterised: u64,
}

impl OutlineGlyphs {
    /// The atlas was made anew: every id is free again.
    pub fn clear(&mut self) {
        self.made.clear();
        self.ids.clear();
        self.fonts.clear();
        self.images.clear();
        self.kept = 0;
    }

    /// Whether the ids are used up: the layer makes its atlas anew before its next prepare. Glyphs with nothing to
    /// stroke count too (the map stays bounded).
    pub fn full(&self) -> bool {
        self.ids.len() >= MAX_IDS
    }

    /// Before a prepare: the images kept go when they are past `KEEP_BYTES` (made again if asked).
    pub fn begin(&mut self) {
        if self.kept > KEEP_BYTES {
            self.images.clear();
            self.kept = 0;
        }
    }

    /// Outline glyphs made since the atlas was last made anew, and outline images made in all (each a swash render).
    pub fn counts(&self) -> (usize, u64) {
        (self.made.len(), self.rasterised)
    }

    /// The outline glyph of the glyph `key`, its outline `width` px wide: its custom-glyph id and where its image sits
    /// from the glyph's origin (swash's placement: `top` up from the baseline). None for a glyph with nothing to draw.
    pub fn glyph(
        &mut self,
        fs: &mut glyphon::FontSystem,
        key: CacheKey,
        width: f32,
    ) -> Option<(u16, Placement)> {
        if let Some(&id) = self.ids.get(&(key, width.to_bits())) {
            return id.map(|id| (id, self.made[usize::from(id)].placement));
        }
        if self.full() {
            return None;
        }
        let font = match self.fonts.get(&key.font_id) {
            Some(font) => font.clone(),
            None => {
                let font = fs.get_font(key.font_id)?;
                self.fonts.insert(key.font_id, font.clone());
                font
            }
        };
        let Some((placement, data)) = rasterise(&mut self.context, &font, key, width) else {
            self.ids.insert((key, width.to_bits()), None);
            return None;
        };
        self.rasterised += 1;
        let id = self.made.len() as u16;
        self.made.push(Made {
            key,
            width,
            placement,
        });
        self.ids.insert((key, width.to_bits()), Some(id));
        self.kept += data.len();
        self.images.insert(id, data);
        Some((id, placement))
    }

    /// glyphon's ask for an outline glyph's image (its first draw, a miss, or the atlas growing): the one kept, or made
    /// again from its font (and kept). Always the size it was asked at (glyphon checks).
    pub fn image(&mut self, ask: RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph> {
        let made = self.made.get(usize::from(ask.id))?;
        let data = match self.images.get(&ask.id) {
            Some(data) => data.clone(),
            None => {
                let font = self.fonts.get(&made.key.font_id)?;
                let data = rasterise(&mut self.context, font, made.key, made.width)?.1;
                self.rasterised += 1;
                self.kept += data.len();
                self.images.insert(ask.id, data.clone());
                data
            }
        };
        let size = usize::from(ask.width) * usize::from(ask.height);
        // Made the same way each time, so always its size; never a wrong-sized image (glyphon would panic on it).
        let data = if data.len() == size {
            data
        } else {
            vec![0; size]
        };
        Some(RasterizedCustomGlyph {
            data,
            content_type: ContentType::Mask,
        })
    }
}

/// The glyph `key` grown by `width` px all round, as an alpha mask, and its placement.
fn rasterise(
    context: &mut ScaleContext,
    font: &Font,
    key: CacheKey,
    width: f32,
) -> Option<(Placement, Vec<u8>)> {
    // As cosmic-text's swash cache scales glyphs for glyphon, so the outline sits exactly round the fill.
    let mut scaler = context
        .builder(font.as_swash())
        .size(f32::from_bits(key.font_size_bits))
        .hint(true)
        .build();
    let offset = Vector::new(key.x_bin.as_float(), key.y_bin.as_float());
    let transform = key
        .flags
        .contains(CacheKeyFlags::FAKE_ITALIC)
        .then(|| Transform::skew(Angle::from_degrees(14.0), Angle::from_degrees(0.0)));
    let fill = Render::new(&[
        Source::ColorOutline(0),
        Source::ColorBitmap(StrikeWith::BestFit),
        Source::Outline,
    ])
    .format(Format::Alpha)
    .offset(offset)
    .transform(transform)
    .render(&mut scaler, key.glyph_id)?;
    let mut stroke = Stroke::new(2.0 * width);
    stroke.join(Join::Round);
    let ring = Render::new(&[Source::Outline])
        .format(Format::Alpha)
        .offset(offset)
        .transform(transform)
        .style(stroke)
        .render(&mut scaler, key.glyph_id)
        .filter(|ring| ring.placement.width > 0 && ring.placement.height > 0);
    match ring {
        Some(ring) => Some(union(ring, &fill)),
        None => grown(&fill, width),
    }
}

/// `fill`'s coverage at pixel `i` of its image.
fn coverage(fill: &Image, i: usize) -> u8 {
    match fill.content {
        Content::Color => fill.data[i * 4 + 3],
        _ => fill.data[i],
    }
}

/// The stroked outline with the fill under it: the max of the two, in the stroke's image (it holds the fill's).
fn union(ring: Image, fill: &Image) -> (Placement, Vec<u8>) {
    let p = ring.placement;
    let mut data = ring.data;
    let f = fill.placement;
    let (dx, dy) = (f.left - p.left, p.top - f.top);
    for y in 0..f.height as i32 {
        let ty = y + dy;
        if ty < 0 || ty >= p.height as i32 {
            continue;
        }
        for x in 0..f.width as i32 {
            let tx = x + dx;
            if tx < 0 || tx >= p.width as i32 {
                continue;
            }
            let at = ty as usize * p.width as usize + tx as usize;
            let c = coverage(fill, y as usize * f.width as usize + x as usize);
            data[at] = data[at].max(c);
        }
    }
    (p, data)
}

/// A glyph with no outline (a colour bitmap): its coverage (from half) grown by `width` px, the edge a pixel soft,
/// with its own coverage under it. Exact Euclidean distances (Felzenszwalb's two-pass transform), O(pixels).
fn grown(fill: &Image, width: f32) -> Option<(Placement, Vec<u8>)> {
    let f = fill.placement;
    if f.width == 0 || f.height == 0 {
        return None;
    }
    let pad = width.ceil() as u32 + 1;
    let (w, h) = ((f.width + 2 * pad) as usize, (f.height + 2 * pad) as usize);
    const FAR: f32 = 1e20;
    let mut d = vec![FAR; w * h];
    let (fw, pad) = (f.width as usize, pad as usize);
    for y in 0..f.height as usize {
        for x in 0..fw {
            if coverage(fill, y * fw + x) >= 128 {
                d[(y + pad) * w + x + pad] = 0.0;
            }
        }
    }
    let n = w.max(h);
    let (mut line, mut out, mut v, mut z) = (
        vec![0.0f32; n],
        vec![0.0f32; n],
        vec![0usize; n],
        vec![0.0f32; n + 1],
    );
    for x in 0..w {
        for y in 0..h {
            line[y] = d[y * w + x];
        }
        squared_distances(&line[..h], &mut out[..h], &mut v, &mut z);
        for y in 0..h {
            d[y * w + x] = out[y];
        }
    }
    for y in 0..h {
        line[..w].copy_from_slice(&d[y * w..(y + 1) * w]);
        squared_distances(&line[..w], &mut out[..w], &mut v, &mut z);
        d[y * w..(y + 1) * w].copy_from_slice(&out[..w]);
    }
    let mut data: Vec<u8> = d
        .iter()
        // A pixel `d` from the nearest covered one's centre is covered as far as the covered square grown by `width`
        // reaches into it.
        .map(|&sq| ((width + 1.0 - sq.sqrt()).clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    for y in 0..f.height as usize {
        for x in 0..fw {
            let at = (y + pad) * w + x + pad;
            data[at] = data[at].max(coverage(fill, y * fw + x));
        }
    }
    let placement = Placement {
        left: f.left - pad as i32,
        top: f.top + pad as i32,
        width: w as u32,
        height: h as u32,
    };
    Some((placement, data))
}

/// The 1-D squared distance transform of `f` (0 inside, far outside) into `d` (Felzenszwalb and Huttenlocher's lower
/// envelope of parabolas); `v` and `z` are scratch of at least `f.len()` and `f.len() + 1`.
fn squared_distances(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        let qf = q as f32;
        let mut s;
        // z[0] is −∞, so the first parabola is never dropped.
        loop {
            let p = v[k] as f32;
            s = ((f[q] + qf * qf) - (f[v[k]] + p * p)) / (2.0 * qf - 2.0 * p);
            if s <= z[k] && k > 0 {
                k -= 1;
            } else {
                break;
            }
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f32::INFINITY;
    }
    let mut k = 0;
    for (q, out) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = q as f32 - v[k] as f32;
        *out = p * p + f[v[k]];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The distance transform finds each point's nearest inside point, both ways along a line.
    #[test]
    fn squared_distances_are_to_the_nearest_inside_point() {
        let far = 1e20;
        let f = [far, far, 0.0, far, far, far, 0.0, far];
        let mut d = [0.0; 8];
        let (mut v, mut z) = ([0usize; 8], [0.0f32; 9]);
        squared_distances(&f, &mut d, &mut v, &mut z);
        assert_eq!(d, [4.0, 1.0, 0.0, 1.0, 4.0, 1.0, 0.0, 1.0]);
    }

    /// A colour glyph's coverage grown by 3 px: a 1-px dot becomes a disc about 7 px across with a soft edge.
    #[test]
    fn a_bitmap_glyph_is_grown_into_a_disc() {
        let dot = Image {
            source: Source::ColorBitmap(StrikeWith::BestFit),
            content: Content::Mask,
            placement: Placement {
                left: 0,
                top: 1,
                width: 1,
                height: 1,
            },
            data: vec![255],
        };
        let (p, data) = grown(&dot, 3.0).unwrap();
        assert_eq!((p.width, p.height, p.left, p.top), (9, 9, -4, 5));
        let at = |x: usize, y: usize| data[y * 9 + x];
        assert_eq!(at(4, 4), 255, "the dot");
        assert_eq!(at(7, 4), 255, "3 px out on the row: inside");
        assert_eq!(at(8, 4), 0, "4 px out: outside");
        assert!(at(6, 6) == 255, "about 2.8 px out on the diagonal: inside");
        assert!(at(7, 7) == 0, "about 4.2 px out on the diagonal: outside");
    }
}
