//! What a scene text node paints with beyond its node's fill: each span's own colour and background, and the node's
//! outline, resolved once with its instance's settings; and what its pieces of one look draw with in a frame, once
//! (`paint_entry`), its baked shadow with it.

use klipolo_model::scene::Paint;
use klipolo_model::scene::instance::{Field, SceneInstance};

use super::place::{GpuClip, World, box_clip};
use super::statics::Look;
use super::text_atlas::Cell;
use super::text_nodes::NodeText;
use super::text_pack::NodeFrame;
use super::text_pipeline::TextPaintDraw;

/// A span's background: a rounded rect under its pieces, padded (1080p pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Background {
    pub color: [f32; 4],
    pub padding: f32,
    pub radius: f32,
}

/// What a text node paints with beyond its node's fill: each look's own colour (None: the node's fill) and background,
/// and its outline (straight RGBA, 1080p pixels wide).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct TextPaint {
    pub fills: Vec<Option<[f32; 4]>>,
    pub backgrounds: Vec<Option<Background>>,
    pub outline: Option<([f32; 4], f32)>,
}

impl TextPaint {
    pub(super) fn of(inst: &SceneInstance, i: usize) -> TextPaint {
        let Some(cn) = inst.cnode(i).and_then(|c| inst.compiled().nodes.get(c)) else {
            return TextPaint::default();
        };
        let p = &cn.node.props;
        let mut paint = TextPaint {
            fills: vec![None],
            backgrounds: vec![None],
            outline: None,
        };
        for s in p.spans.iter().flatten() {
            // A span's gradient draws across its letters (`paint_of`), its first stop's colour when the table is full;
            // a generator inside a span draws the node's fill.
            let fill = match &s.fill {
                Some(Paint::Color(c)) => inst.color_of(i, c),
                Some(Paint::Gradient(g)) => g.stops.first().and_then(|(_, c)| inst.color_of(i, c)),
                _ => None,
            };
            paint.fills.push(fill);
            paint.backgrounds.push(s.background.as_ref().and_then(|bg| {
                Some(Background {
                    color: inst.color_of(i, &bg.color)?,
                    padding: bg.padding.unwrap_or(0.0).clamp(0.0, 1000.0) as f32,
                    radius: bg.radius.unwrap_or(0.0).clamp(0.0, 1000.0) as f32,
                })
            }));
        }
        paint.outline = p.outline.as_ref().and_then(|o| {
            let width = inst.number_of(i, Field::OutlineWidth, &o.width)?;
            Some((inst.color_of(i, &o.color)?, width as f32))
        });
        paint
    }
}

/// What piece of look `look` of node `nt` paints with this frame (`f`): its fill (straight RGBA: an emoji's white, a
/// span's colour, else the node's), its gradient (a span's own across its own letters, else the node's), and
/// the gradient's box, its size and its top left in box pixels.
pub(super) fn paint_of(
    nt: &NodeText,
    f: &NodeFrame,
    look: usize,
) -> ([f32; 4], i32, [f32; 2], [f32; 2]) {
    // An emoji node has no fill: a face without colour glyphs draws its stand-in white.
    let emoji = (f.st.look == Look::Emoji).then_some([1.0; 4]);
    let own = emoji.or(nt.paint.fills.get(look).copied().flatten());
    let span = look
        .checked_sub(1)
        .and_then(|k| f.spans.get(f.st.span_from as usize + k))
        .copied()
        .filter(|&g| g >= 0 && emoji.is_none());
    let fill = match span {
        Some(_) => [1.0; 4],
        None => own.unwrap_or_else(|| match f.st.fill_solid {
            Some(c) => [0, 1, 2, 3].map(|j| c[j] * f.s.fill[j]),
            None => f.s.fill,
        }),
    };
    let gradient = match (span, own) {
        (Some(g), _) => g,
        (None, None) => f.st.fill_gradient,
        (None, Some(_)) => -1,
    };
    match nt.look_boxes.get(look).filter(|_| span.is_some()) {
        Some(b) => (fill, gradient, [b[2] - b[0], b[3] - b[1]], [b[0], b[1]]),
        None => (fill, gradient, [f.here.w as f32, f.here.h as f32], [0.0; 2]),
    }
}

/// The three clips a piece keeps to: its box when its text fits, its node's own and its ancestor's.
pub(super) fn clips(here: &World, fits: bool) -> [[f32; 4]; 9] {
    let own = if fits { box_clip(here) } else { None };
    let [a, b, c] = [own.unwrap_or_default(), here.clips[0], here.clips[1]];
    let gpu = |g: GpuClip| [g.m, g.t, g.c];
    let [a, b, c] = [gpu(a), gpu(b), gpu(c)];
    [a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]]
}

/// What node `nt`'s pieces of look `look` draw with this frame (`f`), once for all of them: its fill, gradient
/// and outline, its clips (`fits`: its text keeps to its box), and its baked shadow when it has one and it shows
/// (`shadow`: its piece's shadow cell; the offset in box pixels, through its text's fit `u`).
pub(super) fn paint_entry(
    nt: &NodeText,
    f: &NodeFrame,
    look: usize,
    fits: bool,
    (shadow, u): (Option<Cell>, f64),
) -> TextPaintDraw {
    let (fill, gradient, size, origin) = paint_of(nt, f, look);
    let sh = f.s.shadow;
    let shows = sh.color[3] > 0.0 && (sh.blur > 0.0 || sh.x != 0.0 || sh.y != 0.0);
    let u = if u.is_finite() && u > 0.0 { u } else { 1.0 };
    let (color, at, cell) = match shadow.filter(|_| shows) {
        Some(c) => {
            let (x, y) = ((sh.x * f.k / u) as f32, (sh.y * f.k / u) as f32);
            (sh.color, [x, y, 0.0, 0.0], c)
        }
        None => ([0.0; 4], [0.0; 4], Cell::default()),
    };
    TextPaintDraw {
        fill,
        outline: nt.paint.outline.map_or([0.0; 4], |(c, _)| c),
        paint: [
            gradient as f32,
            f.s.fill_angle.to_radians() as f32,
            f.s.fill_offset as f32,
            1.0,
        ],
        gbox: [size[0], size[1], origin[0], origin[1]],
        clips: clips(f.here, fits),
        shadow: color,
        shadow_at: at,
        shadow_rect: cell.rect,
        shadow_uv: cell.uv,
        cmap: f.cmap,
    }
}
