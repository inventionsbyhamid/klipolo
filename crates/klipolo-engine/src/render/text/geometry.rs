//! The pure geometry of a text layer's texture (split from `render/text.rs`, no behaviour change): how much bigger a
//! text is laid out, the room around its box, the window of a layout a texture holds, and how soft a text may be laid
//! out to fit the text ceiling. Arithmetic only: no GPU, no shaping.

use crate::plan::TextLayer;

/// Textures are never larger than this on either side.
pub(super) const MAX_SIDE: u32 = 4096;
/// A text growing by at most this much is laid out at its own size (magnified that little, it stays crisp; a pulse
/// shouldn't double its texture).
const GROWS_CRISP: f64 = 1.06;

/// How many times bigger than its box a text whose motion reaches `reach` is laid out: `reach` rounded up to a power
/// of √2 (a title keyed 1× → 3× is laid out 4× its size, once), 1 for a reach of at most `GROWS_CRISP`, and never so
/// big that its texture (the box and its padding, `longest_px` on the longer side at 1×) passes `MAX_SIDE`.
pub fn layout_scale(reach: f64, longest_px: f64) -> f64 {
    if !(reach.is_finite() && reach > GROWS_CRISP) {
        return 1.0;
    }
    // The smallest k with √2^k ≥ reach (less a hair, so a reach of exactly √2 stays √2).
    let k = (2.0 * reach.log2() - 1e-9).ceil();
    let up = 2f64.powf(k / 2.0);
    up.min((f64::from(MAX_SIDE) / longest_px.max(1.0)).max(1.0))
}

/// The room around a text's box in its texture, (each side, top and bottom), for a `font` px font, a `stroke` px
/// outline and a box `box_w` px wide: the background, outline and shadow, and the wrap slack.
pub(super) fn padding(font: f64, stroke: f64, box_w: f64) -> (f64, f64) {
    (
        (font * 0.25 + stroke + box_w * 0.05 + 4.0).ceil(),
        (font * 0.25 + stroke + 4.0).ceil(),
    )
}

/// The softest any text is laid out for the ceiling: a 4096th of its pixels each way, about one texel of a full-size
/// texture (drawn soft rather than dropped). A text stops softening sooner, where its font or box would
/// clamp to a pixel ([`Geometry::softest`]); one that doesn't fit even there isn't drawn, and the frame says so.
pub(super) const MIN_DOWN: f64 = 1.0 / 4096.0;
/// A text's texture is a window of its layout only when that layout shows this much bigger than the frame, so texts
/// on the frame (a slide-in included) keep one texture while they move.
const CLIP_FROM: f64 = 1.5;
/// Room kept around the part of a clipped text that can show, as a share of the frame's longer side, so a moving text
/// keeps its window a while.
const WINDOW_MARGIN: f64 = 0.25;
/// A window's edges snap outward to multiples of this many texture pixels.
const WINDOW_STEP: f64 = 64.0;

/// The part of a layout a texture holds, in the layout's texture pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Window {
    pub(super) x: u32,
    pub(super) y: u32,
    pub(super) w: u32,
    pub(super) h: u32,
}

impl Window {
    pub(super) fn bytes(self, mipmapped: bool) -> u64 {
        let base = u64::from(self.w) * u64::from(self.h) * 4;
        if mipmapped { base + base / 3 } else { base }
    }

    /// The part of `self` inside `other` (None when they don't meet).
    pub(super) fn within(self, other: Window) -> Option<Window> {
        let (x0, y0) = (self.x.max(other.x), self.y.max(other.y));
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        (x1 > x0 && y1 > y0).then_some(Window {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        })
    }

    pub(super) fn contains(self, other: Window) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && other.x + other.w <= self.x + self.w
            && other.y + other.h <= self.y + self.h
    }
}

/// What a text's texture should hold this frame: what can show (`visible`), and the window to cut when the held one
/// doesn't hold that (`cut`, with room around it). The same for a text drawn whole.
#[derive(Clone, Copy)]
pub(super) struct Windows {
    pub(super) visible: Window,
    pub(super) cut: Window,
}

/// What part of a text's whole texture (`full`) to draw this frame: the whole of it, cut evenly to `MAX_SIDE` (as
/// before), unless it shows `CLIP_FROM` times the frame or more (`k` screen pixels per texture pixel). Then only what
/// can show of it (`visible`: the frame's corners taken back through the quad's turn about `centre` and its scale), and
/// a `cut` grown by `WINDOW_MARGIN`, snapped to `WINDOW_STEP` and cut to `MAX_SIDE` about its centre: a held texture is
/// kept while it holds what shows, so a text zooming in is never cut again. None when nothing shows.
pub(super) fn window(
    full: (u32, u32),
    centre: (f64, f64),
    k: f64,
    rotation_deg: f64,
    out: (f64, f64),
) -> Option<Windows> {
    let (fw, fh) = (f64::from(full.0), f64::from(full.1));
    let capped = |x: f64, len: f64| {
        let len = len.max(1.0);
        let side = f64::from(MAX_SIDE).min(len);
        ((x + (len - side) / 2.0).max(0.0) as u32, side as u32)
    };
    let whole = || {
        let ((x, w), (y, h)) = (capped(0.0, fw), capped(0.0, fh));
        let all = Window { x, y, w, h };
        Windows {
            visible: all,
            cut: all,
        }
    };
    if !(k.is_finite() && k > 0.0) || (fw * k <= out.0 * CLIP_FROM && fh * k <= out.1 * CLIP_FROM) {
        return Some(whole());
    }
    let (sin, cos) = (rotation_deg % 360.0).to_radians().sin_cos();
    // image.wgsl's turn, undone: screen offset → texture offset.
    let to_texture = |x: f64, y: f64| {
        let (ox, oy) = (x - centre.0, y - centre.1);
        (
            fw / 2.0 + (cos * ox + sin * oy) / k,
            fh / 2.0 + (-sin * ox + cos * oy) / k,
        )
    };
    let corners = [
        to_texture(0.0, 0.0),
        to_texture(out.0, 0.0),
        to_texture(0.0, out.1),
        to_texture(out.0, out.1),
    ];
    if corners
        .iter()
        .any(|(x, y)| !(x.is_finite() && y.is_finite()))
    {
        return Some(whole());
    }
    let xs = corners.map(|c| c.0);
    let ys = corners.map(|c| c.1);
    let (x_lo, x_hi) = (
        xs.into_iter().fold(f64::INFINITY, f64::min),
        xs.into_iter().fold(f64::NEG_INFINITY, f64::max),
    );
    let (y_lo, y_hi) = (
        ys.into_iter().fold(f64::INFINITY, f64::min),
        ys.into_iter().fold(f64::NEG_INFINITY, f64::max),
    );
    let span = |lo: f64, hi: f64, len: f64| {
        let (a, b) = (lo.floor().clamp(0.0, len), hi.ceil().clamp(0.0, len));
        (b > a).then_some((a as u32, (b - a) as u32))
    };
    let ((vx, vw), (vy, vh)) = (span(x_lo, x_hi, fw)?, span(y_lo, y_hi, fh)?);
    let margin = WINDOW_MARGIN * out.0.max(out.1) / k;
    let lo =
        |v: f64, len: f64| (((v - margin) / WINDOW_STEP).floor() * WINDOW_STEP).clamp(0.0, len);
    let hi = |v: f64, len: f64| (((v + margin) / WINDOW_STEP).ceil() * WINDOW_STEP).clamp(0.0, len);
    let (x0, x1, y0, y1) = (lo(x_lo, fw), hi(x_hi, fw), lo(y_lo, fh), hi(y_hi, fh));
    let ((cx, w), (cy, h)) = (capped(0.0, x1 - x0), capped(0.0, y1 - y0));
    Some(Windows {
        visible: Window {
            x: vx,
            y: vy,
            w: vw,
            h: vh,
        },
        cut: Window {
            x: x0 as u32 + cx,
            y: y0 as u32 + cy,
            w,
            h,
        },
    })
}

/// What a text layer's frame needs before it is laid out: its box and font in output pixels, how much bigger it is laid
/// out (`up`), its centre on screen and the output.
#[derive(Clone)]
pub(super) struct Geometry {
    pub(super) box_w: f64,
    pub(super) box_h: f64,
    pub(super) font: f64,
    pub(super) stroke: f64,
    /// The margin its padded filters draw past it, on every side, in output pixels: room its texture keeps.
    pub(super) margin: f64,
    pub(super) up: f64,
    pub(super) centre: (f64, f64),
    pub(super) out: (f64, f64),
    /// The motion's scale and turn this frame.
    pub(super) scale: f64,
    pub(super) rotation: f64,
}

impl Geometry {
    pub(super) fn of(layer: &TextLayer, canvas: (u32, u32), out: (u32, u32)) -> Self {
        let (ow, oh) = (f64::from(out.0), f64::from(out.1));
        let px_scale = ow.min(oh) / f64::from(canvas.0.min(canvas.1).max(1));
        let (box_w, box_h) = (layer.dest.w * ow, layer.dest.h * oh);
        // The outline scales with the font (which follows the text's box in each format).
        let per_size = if layer.style.size > 0.0 {
            layer.font_px / layer.style.size
        } else {
            f64::from(canvas.0.min(canvas.1)) / 1080.0
        };
        let stroke = layer
            .style
            .outline
            .as_ref()
            .map_or(0.0, |o| o.width * per_size * px_scale);
        let font = layer.font_px * px_scale;
        // 1080p pixels, as the passes' `kl.px` counts them.
        let margin = layer.pad_px.max(0.0) * ow.min(oh) / 1080.0;
        // Capped by the whole texture at 1× (box and padding), so a text laid out bigger still fits it whole. The
        // same on every frame of a layer (its reach doesn't change), so the layout is kept.
        let (pad_x, pad_y) = padding(font, stroke, box_w);
        let (pad_x, pad_y) = (pad_x + margin, pad_y + margin);
        let up = layout_scale(layer.reach, (box_w + 2.0 * pad_x).max(box_h + 2.0 * pad_y));
        let d = layer.dest;
        Self {
            box_w,
            box_h,
            font,
            stroke,
            margin,
            up,
            centre: ((d.x + d.w / 2.0) * ow, (d.y + d.h / 2.0) * oh),
            out: (ow, oh),
            scale: layer.scale,
            rotation: layer.rotation,
        }
    }

    /// The softest it may be laid out: its font and box are drawn at least a pixel, so softer
    /// than where the smallest of them would be under one, it would draw larger than its box. Never softer than
    /// [`MIN_DOWN`]; a text with no size can't be softened at all.
    pub(super) fn softest(&self) -> f64 {
        let smallest = self.font.min(self.box_w).min(self.box_h) * self.up;
        if smallest > 0.0 {
            (1.0 / smallest).clamp(MIN_DOWN, 1.0)
        } else {
            1.0
        }
    }

    /// The font and box in texture pixels at `scale` (texture pixels per box pixel).
    pub(super) fn sizes(&self, scale: f64) -> (u32, (u32, u32)) {
        (
            (self.font * scale).round().max(1.0) as u32,
            (
                (self.box_w * scale).round().max(1.0) as u32,
                (self.box_h * scale).round().max(1.0) as u32,
            ),
        )
    }

    /// The bytes its texture would take at `down` of its full resolution, before laying it out: its box and padding
    /// (lines overflowing the box can make it more), through `window` as the frame places it. Arithmetic only.
    pub(super) fn estimate(&self, down: f64) -> u64 {
        let scale = self.up * down;
        let (font_px, (bw, bh)) = self.sizes(scale);
        let (pad_x, pad_y) = padding(f64::from(font_px), self.stroke * scale, f64::from(bw));
        let (pad_x, pad_y) = (pad_x + self.margin * scale, pad_y + self.margin * scale);
        let full = (
            ((f64::from(bw) + 2.0 * pad_x) as u32).max(1),
            ((f64::from(bh) + 2.0 * pad_y).ceil() as u32).max(1),
        );
        let k = self.scale / scale.max(f64::MIN_POSITIVE);
        window(full, self.centre, k, self.rotation, self.out)
            .map_or(0, |w| w.cut.bytes(scale > 1.0))
    }
}

/// One try at laying a new text out: the share of its full resolution, and whether it is the full-resolution try a
/// text whose box fits the ceiling gets first. The ladder every new title climbs down, on the
/// render thread and on the title build thread alike, so a title built ahead is exactly as sharp as one built cold
///.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Try {
    pub(super) down: f64,
    pub(super) whole_first: bool,
}

impl Try {
    /// The first try: full resolution while there is room and its box fits `open` (the ceiling less what other
    /// drawers hold), as its texture is cut to its ink, which often fits where its box doesn't; else the largest
    /// resolution its estimate fits in `room`. None when nothing fits (refused).
    pub(super) fn first(geometry: &Geometry, room: u64, open: u64) -> Option<Self> {
        if room > 0 && geometry.estimate(1.0) <= open {
            return Some(Self {
                down: 1.0,
                whole_first: true,
            });
        }
        first_fit(geometry, room).map(|down| Self {
            down,
            whole_first: false,
        })
    }

    /// A softened text's retry, once its wait is over: the ladder a new text climbs
    /// (`first`), from its first try sharper than `down`; None when there is none. Full resolution while its box fits
    /// `open` and there is room, its ink then checked against the room, so a text softened while the room was short
    /// gets sharp again once its ink fits, even when its box's estimate never does. A try `known` too big (its
    /// resolution and the bytes it needed) is skipped while the room is still smaller than those bytes, so a
    /// text that stays soft isn't laid out again at full resolution every second.
    pub(super) fn sharper(
        geometry: &Geometry,
        down: f64,
        room: u64,
        open: u64,
        known: Option<(f64, u64)>,
    ) -> Option<Self> {
        let mut t = Self::first(geometry, room, open)?;
        while known.is_some_and(|(at, bytes)| t.down == at && room < bytes) {
            t = t.next(geometry, room)?;
        }
        (t.down > down).then_some(t)
    }

    /// After this try didn't fit in `room` (its ink, or its lines): the next, softer one. After the full-resolution
    /// try, the largest its estimate fits (at most half); else half. None past its softest, or with no room at all.
    pub(super) fn next(self, geometry: &Geometry, room: u64) -> Option<Self> {
        let down = if self.whole_first {
            first_fit(geometry, room)?.min(self.down / 2.0)
        } else {
            self.down / 2.0
        };
        (down >= geometry.softest() && room > 0).then_some(Self {
            down,
            whole_first: false,
        })
    }
}

/// The largest resolution, halving from full, at which `geometry`'s estimate fits `room`, never softer than
/// [`Geometry::softest`]: None when it doesn't fit even there (the text is refused, and the frame says so).
pub(super) fn first_fit(geometry: &Geometry, room: u64) -> Option<f64> {
    let softest = geometry.softest();
    let mut down = 1.0;
    while down / 2.0 >= softest && geometry.estimate(down) > room {
        down /= 2.0;
    }
    (geometry.estimate(down) <= room).then_some(down)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full-frame box at 4K, its text at 72 px (its ink is a line of it).
    fn full_frame() -> Geometry {
        Geometry {
            box_w: 3840.0,
            box_h: 2160.0,
            font: 144.0,
            stroke: 0.0,
            margin: 0.0,
            up: 1.0,
            centre: (1920.0, 1080.0),
            out: (3840.0, 2160.0),
            scale: 1.0,
            rotation: 0.0,
        }
    }

    /// A softened text's retry climbs the ladder a new text climbs (`Try::first`): full
    /// resolution while its box fits the ceiling and there is room, its ink then checked against the room, where it
    /// used to wait for its box's estimate to fit the room, which near the ceiling may never come while it shows.
    #[test]
    fn a_softened_texts_retry_climbs_the_new_texts_ladder() {
        let g = full_frame();
        let full = g.estimate(1.0);
        let (room, open) = (full / 4, full * 2);
        assert_eq!(
            Try::sharper(&g, 0.5, room, open, None),
            Some(Try {
                down: 1.0,
                whole_first: true,
            }),
            "its box past the room, its ink tried"
        );
        // Its box past the ceiling too: the largest resolution its estimate fits, if that is sharper.
        let fit = first_fit(&g, room).expect("fits softer");
        assert_eq!(
            Try::sharper(&g, fit / 4.0, room, full - 1, None),
            Some(Try {
                down: fit,
                whole_first: false,
            })
        );
        assert_eq!(
            Try::sharper(&g, fit, room, full - 1, None),
            None,
            "nothing sharper fits"
        );
        assert_eq!(
            Try::sharper(&g, 1.0, room, open, None),
            None,
            "already sharp"
        );
    }

    /// A full-resolution retry already known too big for the room (it needed more
    /// bytes than the room holds) isn't laid out again while the room is still that small; once the room holds it, it
    /// is tried again.
    #[test]
    fn a_retry_known_too_big_waits_for_the_room_to_hold_it() {
        let g = full_frame();
        let full = g.estimate(1.0);
        let (room, open) = (full / 4, full * 2);
        let fit = first_fit(&g, room).expect("fits softer");
        let known = Some((1.0, room + 1));
        assert_eq!(
            Try::sharper(&g, fit, room, open, known),
            None,
            "its ink didn't fit this room: no full-resolution layout again"
        );
        assert_eq!(
            Try::sharper(&g, fit, room + 1, open, known),
            Some(Try {
                down: 1.0,
                whole_first: true,
            }),
            "the room holds it now: tried again"
        );
    }

    /// A text is never laid out softer than where its font or box would clamp to a pixel (it
    /// would draw larger than its box). One that doesn't fit even there is refused, which the frame says, so an export
    /// stops rather than finish with it wrong.
    #[test]
    fn a_text_is_never_softened_past_where_its_sizes_clamp() {
        let g = Geometry {
            box_w: 1600.0,
            box_h: 400.0,
            font: 16.0,
            stroke: 0.0,
            margin: 0.0,
            up: 1.0,
            centre: (960.0, 540.0),
            out: (1920.0, 1080.0),
            scale: 1.0,
            rotation: 0.0,
        };
        let softest = g.softest();
        assert_eq!(
            softest,
            1.0 / 16.0,
            "where its 16 px font reaches one pixel"
        );
        assert_eq!(
            g.sizes(g.up * softest),
            (1, (100, 25)),
            "nothing clamped at the softest"
        );
        let at_softest = g.estimate(softest);
        assert_eq!(first_fit(&g, at_softest), Some(softest));
        assert_eq!(
            first_fit(&g, at_softest - 1),
            None,
            "refused rather than drawn softer"
        );
        assert!(
            g.estimate(MIN_DOWN) < at_softest,
            "the old floor would have drawn it, larger than its box"
        );
    }
}
