//! A cursor layer as the pass's entries (`cursor.wgsl` reads them): its slot's header (the footage's box, corners and
//! shape mask in target pixels), its ripples and the cursor itself. Pure arithmetic on the planned layer: no GPU, no
//! allocation.

use klipolo_model::layout::BoxN;

use super::Instance;
use crate::plan::{CursorLayer, CursorMask, Ripple};

/// The kinds `cursor.wgsl` draws.
const SHAPE: f32 = 0.0;
const DOT: f32 = 1.0;
const ARROW: f32 = 2.0;
const RIPPLE: f32 = 3.0;
/// Klipolo's arrow is 0.57 of its height wide (`cursor.wgsl`'s polygon); its quad 0.6.
const ARROW_WIDTH: f32 = 0.6;
/// The dot's diameter, in cursor heights.
pub const DOT_SIZE: f64 = 0.6;

/// What draws for the cursor this frame: a resident shape (its rectangle in the atlas), or one of Klipolo's looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    Shape {
        rect: [f32; 4],
        hotspot: (f32, f32),
        aspect: f32,
    },
    Dot,
    Arrow,
}

fn px(b: BoxN, (w, h): (f32, f32)) -> [f32; 4] {
    let (x0, y0) = (b.x as f32 * w, b.y as f32 * h);
    [x0, y0, x0 + b.w as f32 * w, y0 + b.h as f32 * h]
}

fn size(target: (u32, u32)) -> (f32, f32) {
    (target.0 as f32, target.1 as f32)
}

/// The scissor rectangle (x, y, width, height) of `layer`'s box in a `target`-sized pass; None if it shows nothing.
pub fn scissor(layer: &CursorLayer, target: (u32, u32)) -> Option<[u32; 4]> {
    let [x0, y0, x1, y1] = px(layer.clip, size(target));
    let clamp = |v: f32, max: u32| (v.max(0.0) as u32).min(max);
    let (x0, y0) = (clamp(x0.floor(), target.0), clamp(y0.floor(), target.1));
    let (x1, y1) = (clamp(x1.ceil(), target.0), clamp(y1.ceil(), target.1));
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}

/// The slot's header: the box, its corner radius, its shape mask (`mask.wgsl`'s rules, in pixels) and the target.
pub fn header(layer: &CursorLayer, target: (u32, u32)) -> Instance {
    let t = size(target);
    let b = px(layer.clip, t);
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    let side = w.min(h);
    let radius = layer.radius.clamp(0.0, 0.5) as f32 * side;
    let mut place = [radius, -1.0, 0.0, 0.0];
    let mut image = [0.0; 4];
    if let Some(m) = layer.mask {
        let (hw, hh, corner) = mask_shape(m, w, h);
        let room = |at: f32, len: f32, half: f32| at.clamp(0.0, 1.0) * (len - 2.0 * half);
        let centre = (
            b[0] + hw + room(m.center.0, w, hw),
            b[1] + hh + room(m.center.1, h, hh),
        );
        let feather = m.feather.max(0.0) * t.0.min(t.1) / 1080.0;
        place = [radius, m.shape as f32, feather, corner];
        image = [centre.0, centre.1, hw, hh];
    }
    Instance {
        head: b,
        place,
        image,
        more: [t.0, t.1, 0.0, 0.0],
    }
}

/// The mask's half size and corner radius in pixels, for a box `w` × `h`.
fn mask_shape(m: CursorMask, w: f32, h: f32) -> (f32, f32, f32) {
    let side = w.min(h);
    let corner = match m.shape {
        0 => 0.5,
        2 => 0.0,
        _ => m.corner.clamp(0.0, 0.5),
    } * side;
    if m.shape == 3 {
        (w / 2.0, h / 2.0, corner)
    } else {
        (side / 2.0, side / 2.0, corner)
    }
}

/// A ring after a click, in the slot whose header is entry `header`.
pub fn ripple(r: &Ripple, header: usize, target: (u32, u32)) -> Instance {
    let t = size(target);
    let [cr, cg, cb, _] = r.color.map(|c| f32::from(c) / 255.0);
    Instance {
        head: [RIPPLE, header as f32, r.alpha.clamp(0.0, 1.0) as f32, 0.0],
        place: [
            r.center.0 as f32 * t.0,
            r.center.1 as f32 * t.1,
            r.radius.max(0.0) as f32 * t.1,
            0.0,
        ],
        image: [cr, cg, cb, 0.0],
        more: [0.0; 4],
    }
}

/// The cursor drawn as `look`, pressed and smeared, in the slot whose header is entry `header`.
pub fn cursor(layer: &CursorLayer, look: Look, header: usize, target: (u32, u32)) -> Instance {
    let t = size(target);
    let height = (layer.height * layer.press) as f32 * t.1;
    // Kind, the drawn height and width, the atlas rectangle, the hotspot in the image.
    let (kind, h, w, rect, hotspot) = match look {
        Look::Shape {
            rect,
            hotspot,
            aspect,
        } => (SHAPE, height, height * aspect, rect, hotspot),
        Look::Dot => {
            let d = height * DOT_SIZE as f32;
            (DOT, d, d, [0.0; 4], (0.5, 0.5))
        }
        Look::Arrow => (ARROW, height, height * ARROW_WIDTH, [0.0; 4], (0.0, 0.0)),
    };
    Instance {
        head: [
            kind,
            header as f32,
            layer.opacity.clamp(0.0, 1.0) as f32,
            0.0,
        ],
        place: [
            layer.hotspot.0 as f32 * t.0,
            layer.hotspot.1 as f32 * t.1,
            h,
            w,
        ],
        image: rect,
        more: [
            hotspot.0,
            hotspot.1,
            layer.blur.0 as f32 * t.0,
            layer.blur.1 as f32 * t.1,
        ],
    }
}
