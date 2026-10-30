//! The compositor's scene layers: a motion graphic drawn into its own target by the scene drawer
//! (`render::scene`) at the layer's drawn size (its motion's reach included, as a generator's is), then run through the
//! layer's effects (padded ones in a grown target, as a generator's are: `padded.rs`) and placed by the image quad with
//! the layer's rotation, opacity, radius and blend; one that draws only part of its layer (a lower third in a
//! full-frame layer) draws into a target of that part, placed as that part of the quad (`part_of`). Its images are decoded by the image cache first (an export waits
//! for them), and the playback lookahead prepares a scene before its first frame (`warm_scene`). Its text is laid out
//! for the largest scale the layer's motion reaches (`TextAt`), on the scene drawer's text build thread:
//! the preview never waits for it, an export does, and so does the render worker, whose builds take the
//! fonts it was given and never wait for the font scan. A layer scaled to under half a pixel draws nothing.
//! One with a backdrop waits for the frame under it (`backdrop.rs`).

use std::path::Path;

use super::frame::FrameCtx;
use super::staged::effect_size;
use super::{Compositor, Draw};
use crate::plan::SceneLayer;
use crate::render::effects::FrameInfo;
use crate::render::image_load::ImageError;
use crate::render::scene::{SceneAt, SceneStats, TextAt};
use crate::render::uniforms::quad_for;
use crate::vendor::cap::compat::XY;
use crate::vendor::cap::rendering::layers::image::ImageQuad;

impl Compositor {
    /// Scene layer `l`: drawn, through its effects, placed.
    pub(super) async fn scene(
        &mut self,
        f: &mut FrameCtx<'_>,
        draws: &mut Vec<Draw>,
        index: usize,
        l: &SceneLayer,
    ) {
        let dev = self.dev.clone();
        let (device, queue) = (&dev.device, &dev.queue);
        let quad = ImageQuad {
            blend: l.blend,
            ..quad_for(l.dest, l.rotation, l.opacity, l.radius, f.size)
        };
        if quad.size.x < 0.5 || quad.size.y < 0.5 {
            // Scaled to nothing (a pop from 0): nothing shows, so nothing is drawn or laid out.
            return;
        }
        let at = effect_size(&quad, l.reach);
        // Its images, decoded for its size (the preview draws without one still decoding; export waits).
        let side = at.size.0.max(at.size.1);
        let mut textures = std::mem::take(self.scenes.textures());
        textures.clear();
        for (_, path) in l.images.iter() {
            let texture = match self
                .quads
                .texture(device, queue, Path::new(&**path), side, f.wait)
                .await
            {
                Ok(texture) => texture,
                Err(ImageError::Missing(_)) => {
                    f.missing.push(path.clone());
                    None
                }
                Err(error) => {
                    f.warnings.push(error.to_string());
                    None
                }
            };
            textures.push(texture);
        }
        *self.scenes.textures() = textures;
        let info = at.info(f.info);
        let place = SceneAt {
            canvas: f.canvas,
            size: at.size,
        };
        let text = text_at(f.info.px, l, at.cap, (self.fonts_wait(f.wait), f.wait));
        if l.backdrop {
            // It reads the frame under it: drawn when the passes reach it (`backdrop.rs`).
            self.defer_backdrop(draws, index, quad, at, place, text);
            return;
        }
        let Some(drawn) = self.scenes.draw(
            device,
            queue,
            f.encoder,
            &mut self.effects,
            text,
            l,
            place,
            info,
            None,
            f.warnings,
        ) else {
            return;
        };
        if drawn.rect == [0, 0, at.size.0, at.size.1] {
            self.through_padded(
                f,
                draws,
                drawn.target,
                at,
                &l.effects,
                quad,
                l.pad_px,
                l.reach,
            );
        } else {
            // Only what it drew (it has no effects, corners or backdrop): that part of its quad.
            let texture = self.effects.texture(drawn.target).clone();
            self.place(f, draws, &texture, part_of(quad, at.size, drawn.rect));
        }
    }

    /// Prepares scene layer `l` of a `canvas`-sized frame for the last output size (the playback lookahead): its
    /// instance and buffers are built now, not on its first frame; its images start decoding.
    pub(super) fn warm_scene(&mut self, l: &SceneLayer, canvas: (u32, u32)) {
        let (device, queue) = (&self.dev.device, &self.dev.queue);
        let quad = quad_for(l.dest, l.rotation, l.opacity, l.radius, self.last_size);
        if quad.size.x < 0.5 || quad.size.y < 0.5 {
            // Scaled to nothing where it starts (a pop or a key from 0): its frames draw nothing, so nothing is
            // prepared for a 1 × 1 target; its first visible frame builds.
            return;
        }
        let at = effect_size(&quad, l.reach);
        for (_, path) in l.images.iter() {
            self.quads
                .warm(device, queue, Path::new(&**path), at.size.0.max(at.size.1));
        }
        // The frame's pixels per 1080p pixel, as the frame will have them (`draw.rs`).
        let frame = self.last_size.0.min(self.last_size.1) as f32 / 1080.0;
        let place = SceneAt {
            canvas,
            size: at.size,
        };
        // Never waited for here: the lookahead only asks for the text's build (an export's with its fonts waiting).
        let fonts = self.fonts_wait(self.wait_for_assets);
        let text = text_at(frame, l, at.cap, (fonts, false));
        // Its shapes at its text's scale (`SceneDrawer::draw`).
        let px = crate::render::scene::drawn_k(text, at.info(FrameInfo::new(0, frame))) as f32;
        self.scenes.queue_passes(&mut self.effects);
        self.scenes
            .prepare(device, queue, &mut self.effects, text, l, place, px);
    }

    /// What the scene drawer did (tests and the performance numbers).
    #[doc(hidden)]
    pub fn scene_stats(&self) -> SceneStats {
        self.scenes.stats()
    }

    /// Whether a scene text build for a frame that waits (`wait`) waits for this computer's fonts: an export's does
    /// the worker's takes the fonts it was given, as its titles do, so a request never pays for the scan.
    fn fonts_wait(&self, wait: bool) -> bool {
        wait && !self.fonts_given
    }

    /// The scene drawer, for its test hooks.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn scene_drawer(&mut self) -> &mut crate::render::scene::SceneDrawer {
        &mut self.scenes
    }
}

/// The part `rect` (x, y, w, h) of a layer target of `at` pixels that `quad` places whole, as its own quad: the same
/// turn about the layer's centre, opacity and blend, so its pixels land exactly where the whole target's would.
pub(super) fn part_of(quad: ImageQuad, at: (u32, u32), rect: [u32; 4]) -> ImageQuad {
    let (aw, ah) = (f64::from(at.0.max(1)), f64::from(at.1.max(1)));
    let (fx, fy) = (quad.size.x / aw, quad.size.y / ah);
    let [x, y, w, h] = rect.map(f64::from);
    let (lx, ly) = ((x + w / 2.0 - aw / 2.0) * fx, (y + h / 2.0 - ah / 2.0) * fy);
    let (sin, cos) = (quad.rotation_deg % 360.0).to_radians().sin_cos();
    ImageQuad {
        center: XY::new(
            quad.center.x + cos * lx - sin * ly,
            quad.center.y + sin * lx + cos * ly,
        ),
        size: XY::new(w * fx, h * fy),
        ..quad
    }
}

/// How scene layer `l`'s text is laid out on a frame of `frame_px` pixels per 1080p pixel whose target was shrunk by
/// `cap`: for the largest scale its motion reaches, the same every frame, so a scaling layer draws it scaled.
/// `wait`: whether its build waits for this computer's fonts; `block`: whether the frame waits for its build (an
/// export's and the worker's).
fn text_at(frame_px: f32, l: &SceneLayer, cap: f64, (wait, block): (bool, bool)) -> TextAt {
    let reach = if l.motion_reach.is_finite() {
        l.motion_reach.max(1.0)
    } else {
        1.0
    };
    TextAt {
        k: f64::from(frame_px) * reach * cap,
        wait,
        block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(rotation: f64) -> ImageQuad {
        ImageQuad {
            center: XY::new(960.0, 540.0),
            size: XY::new(1920.0, 1080.0),
            rotation_deg: rotation,
            opacity: 0.8,
            rounding: 0.0,
            source_rotation: 0,
            mirror: false,
            blend: klipolo_model::motion::Blend::Screen,
        }
    }

    /// A lower third's own target in a full-frame layer is placed where its pixels stood in the layer's target,
    /// pixel for pixel; turned with the layer about the layer's centre, and drawn at the layer's scale.
    #[test]
    fn a_part_of_a_layer_lands_where_it_stood() {
        let part = part_of(quad(0.0), (1920, 1080), [74, 668, 1168, 288]);
        assert_eq!((part.center.x, part.center.y), (658.0, 812.0));
        assert_eq!((part.size.x, part.size.y), (1168.0, 288.0));
        assert_eq!(
            (part.opacity, part.blend),
            (0.8, klipolo_model::motion::Blend::Screen)
        );
        let turned = part_of(quad(90.0), (1920, 1080), [74, 668, 1168, 288]);
        // (−302, 272) from the centre, turned a quarter clockwise on screen: (−272, −302).
        assert!(
            (turned.center.x - (960.0 - 272.0)).abs() < 1e-9,
            "{:?}",
            turned.center
        );
        assert!(
            (turned.center.y - (540.0 - 302.0)).abs() < 1e-9,
            "{:?}",
            turned.center
        );
        let scaled = part_of(
            ImageQuad {
                size: XY::new(960.0, 540.0),
                ..quad(0.0)
            },
            (1920, 1080),
            [0, 0, 960, 540],
        );
        assert_eq!((scaled.center.x, scaled.center.y), (720.0, 405.0));
        assert_eq!((scaled.size.x, scaled.size.y), (480.0, 270.0));
    }
}
