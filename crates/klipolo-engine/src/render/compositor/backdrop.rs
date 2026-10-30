//! Scene layers with a backdrop: a glass card blurs what is under its layer, so the layer is drawn
//! only once the frame under it is: like an adjustment layer, it ends the pass (`Draw::Backdrop`), its box (only the part
//! its backdrop nodes read, once its timeline was surveyed) is grabbed from the frame drawn so far
//! (`EffectStage::grab`, `Input::View`: a layer moving grabs its moving box each frame),
//! and the scene is drawn with it (`render::scene::Backdrop`: how its target's pixels map onto the grab), run through
//! its effects and placed by its quad for the next pass. The grab keeps one size through the layer's motion
//! (`grab_rect`), so a moving glass card makes no targets or bind groups once warm. Its images are
//! decoded when it is prepared, in its place, as any scene layer's; what it needs then is kept here, in records reused
//! frame after frame (no allocation once warm).

use super::frame::FrameCtx;
use super::staged::EffectSize;
use super::{Compositor, Draw};
use crate::plan::{Layer, SceneLayer};
use crate::render::effects::{FrameInfo, Input, MAX_EFFECT_SIDE, PixelRect};
use crate::render::scene::{Backdrop, SceneAt, TextAt};
use crate::vendor::cap::rendering::layers::image::ImageQuad;

/// A scene layer waiting for the frame under it: its index in its list of layers, its decoded images, where it lands,
/// its target and its text's layout.
pub(super) struct Backdropped {
    index: usize,
    textures: Vec<Option<wgpu::Texture>>,
    quad: ImageQuad,
    at: EffectSize,
    place: SceneAt,
    text: TextAt,
}

impl Compositor {
    /// Scene layer `l` (`index` in its list), whose backdrop reads the frame under it, kept for the passes (its images
    /// decoded into the scene drawer's `textures()` already): `Draw::Backdrop` ends the pass there.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn defer_backdrop(
        &mut self,
        draws: &mut Vec<Draw>,
        index: usize,
        quad: ImageQuad,
        at: EffectSize,
        place: SceneAt,
        text: TextAt,
    ) {
        let k = self.backdrops_used;
        if self.backdrops.len() == k {
            self.backdrops.push(Backdropped {
                index,
                textures: Vec::new(),
                quad,
                at,
                place,
                text,
            });
        }
        let r = &mut self.backdrops[k];
        (r.index, r.quad, r.at, r.place, r.text) = (index, quad, at, place, text);
        std::mem::swap(&mut r.textures, self.scenes.textures());
        self.backdrops_used += 1;
        draws.push(Draw::Backdrop(k));
    }

    /// The scene kept as `k`, drawn now over `frame` (`size` pixels, drawn so far) with what is under its box, through
    /// its effects, as a quad draw for the next pass; None when it draws nothing (its target refused, scaled away).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn backdrop_scene(
        &mut self,
        f: &mut FrameCtx<'_>,
        frame: &wgpu::TextureView,
        k: usize,
        layers: &[Layer],
    ) -> Option<usize> {
        let dev = self.dev.clone();
        let (device, queue) = (&dev.device, &dev.queue);
        let r = self.backdrops.get_mut(k)?;
        let (quad, at, place, text) = (r.quad, r.at, r.place, r.text);
        let Some(Layer::Scene(l)) = layers.get(r.index) else {
            return None;
        };
        std::mem::swap(&mut r.textures, self.scenes.textures());
        let size = f.size;
        // Only the part its backdrop nodes read, once its timeline was surveyed: that part of its quad.
        let reads = match self.scenes.backdrop_box(l, at.size) {
            Some(part) => super::scene::part_of(quad, at.size, part),
            None => quad,
        };
        let rect = grab_rect(&reads, (l.reach, l.turns), size);
        let grabbed = match rect {
            Some(rect) => self
                .effects
                .grab(
                    device, queue, f.encoder, frame, rect, f.info, f.warnings, false,
                )
                .ok()
                .flatten()
                .map(|target| Backdrop {
                    target,
                    map: map(&quad, at.size, rect),
                    px: f.info.px,
                }),
            None => None,
        };
        let info = at.info(f.info);
        let drawn = self.scenes.draw(
            device,
            queue,
            f.encoder,
            &mut self.effects,
            text,
            l,
            place,
            info,
            grabbed,
            f.warnings,
        );
        if let Some(r) = self.backdrops.get_mut(k) {
            std::mem::swap(&mut r.textures, self.scenes.textures());
        }
        let drawn = drawn?;
        self.place_backdropped(f, l, drawn.target, at, quad, info)
    }

    /// A backdrop scene's target `drawn` through its layer's effects (at its own size: a padded filter draws within the
    /// layer here), placed by `quad`.
    fn place_backdropped(
        &mut self,
        f: &mut FrameCtx<'_>,
        l: &SceneLayer,
        drawn: usize,
        at: EffectSize,
        quad: ImageQuad,
        info: FrameInfo,
    ) -> Option<usize> {
        let dev = self.dev.clone();
        let (device, queue) = (&dev.device, &dev.queue);
        let out = self.effects.apply(
            device,
            queue,
            f.encoder,
            Input::Target(drawn),
            at.size,
            &l.effects,
            info,
            f.warnings,
        );
        let shown = match out {
            Ok(made) => made.unwrap_or(drawn),
            Err(_) => return None,
        };
        let texture = self.effects.texture(shown).clone();
        self.quads.push_draw(device, queue, &texture, quad, f.size)
    }
}

/// The frame's box a backdrop layer grabs: the box its quad covers at the largest scale its motion
/// reaches (`reach` times this frame's size, as its target is sized: the same every frame of a pop, a pulse or a
/// slide), in whole pixels, at most the frame (and the largest target), shifted to lie inside the frame rather than
/// cut, so its grab and the copies blurred from it keep one size and the pool hands out the same targets. Turned at a
/// fixed angle it is the turned box's own bounds; only a layer whose motion `turns` it (a spin, a
/// wiggle) takes the box's diagonal both ways, on every frame, so its size never flips on a quarter turn.
fn grab_rect(
    quad: &ImageQuad,
    (reach, turns): (f64, bool),
    frame: (u32, u32),
) -> Option<PixelRect> {
    let reach = if reach.is_finite() && reach > 1.0 {
        reach
    } else {
        1.0
    };
    let grown = |v: f64| (v * reach * 1000.0).round() / 1000.0;
    let (w, h) = (grown(quad.size.x), grown(quad.size.y));
    let turn = quad.rotation_deg.rem_euclid(360.0);
    let (cw, ch) = match turn {
        _ if turns => {
            let d = (w * w + h * h).sqrt();
            (d, d)
        }
        t if t == 0.0 || t == 180.0 => (w, h),
        t if t == 90.0 || t == 270.0 => (h, w),
        t => {
            let (sin, cos) = t.to_radians().sin_cos();
            let (sin, cos) = (sin.abs(), cos.abs());
            (w * cos + h * sin, w * sin + h * cos)
        }
    };
    let (fw, fh) = (f64::from(frame.0), f64::from(frame.1));
    let side = |v: f64, f: f64| {
        let v = if v.is_finite() { v.ceil() } else { f };
        v.clamp(1.0, f.min(f64::from(MAX_EFFECT_SIDE)).max(1.0))
    };
    let (rw, rh) = (side(cw, fw), side(ch, fh));
    let inside = |c: f64, r: f64, f: f64| {
        let v = (c - r / 2.0).floor();
        if v.is_finite() {
            v.clamp(0.0, (f - r).max(0.0))
        } else {
            0.0
        }
    };
    let (x, y) = (inside(quad.center.x, rw, fw), inside(quad.center.y, rh, fh));
    (fw >= 1.0 && fh >= 1.0).then_some(PixelRect {
        x: x as u32,
        y: y as u32,
        w: rw as u32,
        h: rh as u32,
    })
}

/// How a scene target of `size` pixels, placed by `quad`, maps onto `rect` of the frame (the grab): `[a, b, c, d, e,
/// f]` with u = a x + b y + e, v = c x + d y + f, from a target pixel (x, y) to the grab's uv.
fn map(quad: &ImageQuad, size: (u32, u32), rect: PixelRect) -> [f32; 6] {
    let (sw, sh) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    let (qw, qh) = (quad.size.x, quad.size.y);
    let (sx, sy) = (qw / sw, qh / sh);
    let (sin, cos) = quad.rotation_deg.to_radians().sin_cos();
    let (rx, ry) = (f64::from(rect.x), f64::from(rect.y));
    let (rw, rh) = (f64::from(rect.w.max(1)), f64::from(rect.h.max(1)));
    let (cx, cy) = (quad.center.x, quad.center.y);
    [
        cos * sx / rw,
        -sin * sy / rw,
        sin * sx / rh,
        cos * sy / rh,
        (cx - cos * qw / 2.0 + sin * qh / 2.0 - rx) / rw,
        (cy - sin * qw / 2.0 - cos * qh / 2.0 - ry) / rh,
    ]
    .map(|v| v as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendor::cap::compat::XY;
    use klipolo_model::motion::Blend;

    /// A 576 × 72 card at the middle of a 640 × 360 frame, turned `deg`°.
    fn card(deg: f64) -> ImageQuad {
        ImageQuad {
            center: XY { x: 320.0, y: 180.0 },
            size: XY { x: 576.0, y: 72.0 },
            rotation_deg: deg,
            opacity: 1.0,
            rounding: 0.0,
            source_rotation: 0,
            mirror: false,
            blend: Blend::Normal,
        }
    }

    /// Held at 3° the grab is the turned card's own box, not its diagonal square; a card whose motion turns it
    /// keeps the diagonal on every frame, a quarter turn's among them, so its size never flips.
    #[test]
    fn a_still_turn_grabs_its_turned_box_and_a_spin_its_square() {
        let frame = (640, 360);
        let still = grab_rect(&card(3.0), (1.0, false), frame).unwrap();
        assert_eq!((still.w, still.h), (579, 103), "the turned box: {still:?}");
        let sizes: Vec<(u32, u32)> = [0.0, 37.0, 90.0, 180.0, 270.0]
            .iter()
            .map(|&d| grab_rect(&card(d), (1.0, true), frame).unwrap())
            .map(|r| (r.w, r.h))
            .collect();
        assert!(sizes.iter().all(|&s| s == (581, 360)), "{sizes:?}");
        let upright = grab_rect(&card(90.0), (1.0, false), frame).unwrap();
        assert_eq!((upright.w, upright.h), (72, 360), "a still quarter turn");
    }
}
