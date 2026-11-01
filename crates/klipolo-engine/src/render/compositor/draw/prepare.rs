//! One layer prepared for drawing (`prepare_one`): its textures uploaded, its effects run, its quad placed, and its
//! entry pushed onto the frame's draw list.

use std::path::Path;

use super::super::frame::FrameCtx;
use super::super::{Compositor, Draw, staged};
use crate::plan::{Layer, VideoRole};
use crate::render::captions::CaptionDrawer;
use crate::render::image_load::ImageError;
use crate::render::pool::DecodedSet;
use crate::render::shared_fonts::SharedFonts;
use crate::render::text::TextDrawer;
use crate::render::text_timing::TextTiming;
use crate::render::uniforms::quad_for;
use crate::render::video::VideoDraw;
use crate::vendor::cap::rendering::layers::background::BackgroundLayer;
use crate::vendor::cap::rendering::layers::image::ImageQuad;

impl Compositor {
    /// Prepares one layer (`index` in its list) into `draws`; never a transition (a side holds one clip's layers).
    pub(in crate::render::compositor) async fn prepare_one(
        &mut self,
        index: usize,
        layer: &Layer,
        frames: &DecodedSet,
        f: &mut FrameCtx<'_>,
        draws: &mut Vec<Draw>,
    ) {
        let dev = self.dev.clone();
        let (device, queue) = (&dev.device, &dev.queue);
        match layer {
            Layer::Video(v) if v.role == VideoRole::Main && !v.effects.is_empty() => {
                let decoded = frames.get(f.video);
                f.video += 1;
                let Some((frame, rotation)) = decoded else {
                    return;
                };
                self.main_with_effects(f, draws, v, frame, *rotation).await;
            }
            Layer::Video(v) => {
                let decoded = frames.get(f.video);
                f.video += 1;
                let Some((frame, rotation)) = decoded else {
                    return;
                };
                let prepared = self.video.prepare(
                    device,
                    queue,
                    f.encoder,
                    v,
                    frame,
                    *rotation,
                    f.size,
                    f.grain_frame,
                );
                match prepared {
                    Some(VideoDraw::Composite(index)) => {
                        let lut = v.look.as_ref().and_then(|l| l.lut.as_ref());
                        let group = self
                            .luts
                            .bind(
                                device,
                                queue,
                                &self.video.composite,
                                lut,
                                f.warnings,
                                f.wait,
                            )
                            .await;
                        draws.push(Draw::Video(index, group));
                    }
                    Some(VideoDraw::Quad(texture, quad)) => {
                        // The texture is in the frame's stored orientation; effects run on it turned upright, at the
                        // size it's shown (`push_quad`), or the largest its motion reaches, grown by any margin its
                        // filters draw past it (`push_padded`).
                        let at = staged::effect_size(&quad, v.reach);
                        let (fx, pad) = (&v.effects, v.pad_px);
                        self.push_padded(f, draws, &texture, at, fx, quad, pad, v.reach);
                    }
                    None => {}
                }
            }
            Layer::Image(image) => {
                let quad = ImageQuad {
                    blend: image.blend,
                    ..quad_for(
                        image.dest,
                        image.rotation,
                        image.opacity,
                        image.radius,
                        f.size,
                    )
                };
                // Decoded for the largest box its motion reaches, so a scaling image decodes once.
                let side = (quad.size.x.max(quad.size.y) * image.reach).ceil() as u32;
                match self
                    .quads
                    .texture(device, queue, Path::new(&*image.path), side, f.wait)
                    .await
                {
                    Ok(Some(texture)) => {
                        let at = staged::effect_size(&quad, image.reach);
                        let (fx, pad) = (&image.effects, image.pad_px);
                        self.push_padded(f, draws, &texture, at, fx, quad, pad, image.reach);
                    }
                    // Still decoding: this frame draws without it and the redraw signal fires when it's ready.
                    Ok(None) => {}
                    // Moved, renamed or its drive removed: said as a missing video is.
                    Err(ImageError::Missing(_)) => f.missing.push(image.path.clone()),
                    Err(error) => f.warnings.push(error.to_string()),
                }
            }
            Layer::Text(text) => {
                let timed = self.text_timing_starts(draws);
                let wait = f.wait;
                let fonts = self.fonts.get_or_insert_with(|| SharedFonts::new(wait));
                let texts = self.texts.get_or_insert_with(|| {
                    let mut texts = TextDrawer::shared(device, queue).for_export(wait);
                    texts.time_gpu(self.text_timing.as_ref().and_then(TextTiming::stamps));
                    texts
                });
                if let Some((texture, quad, up)) =
                    texts.prepare_with(device, queue, fonts, text, f.canvas, f.size, f.warnings)
                {
                    // Processed at the texture's size (laid out `up` times its box); px settings follow the texture
                    // pixels per drawn pixel, as a moving image's do, so they keep their size on the drawn text.
                    let per_drawn = if text.scale > 1e-9 {
                        up / text.scale
                    } else {
                        1.0
                    };
                    // A filter's margin is room in the texture: shapes and centres keep to the text's own part of it.
                    let size = (texture.width(), texture.height());
                    let own = super::super::padded::text_content(size, text.pad_px, f.info.px, up);
                    let at = staged::EffectSize::texture(size, per_drawn).with_content(own);
                    self.push_quad(f, draws, &texture, at, &text.effects, quad);
                }
                self.text_timing_ends(draws, timed);
            }
            // The caption drawer never builds here: its build thread does (it has a font system of its own).
            Layer::Caption(c) => {
                let timed = self.text_timing_starts(draws);
                let (dev, redraw, wait) = (&self.dev, &self.redraw, f.wait);
                let captions = self
                    .captions
                    .get_or_insert_with(|| CaptionDrawer::new(dev, Some(redraw.clone()), wait));
                if let Some(slot) =
                    captions.prepare(device, queue, f.encoder, c, f.size, f.warnings, wait)
                {
                    draws.push(Draw::Caption(slot));
                }
                self.text_timing_ends(draws, timed);
            }
            Layer::Generator(g) => self.generator(f, draws, g),
            Layer::Scene(l) => self.scene(f, draws, index, l).await,
            Layer::Adjustment(_) => draws.push(Draw::Adjust(index)),
            Layer::Black { alpha } => {
                if self.blacks.len() == f.blacks {
                    self.blacks.push(BackgroundLayer::new(device));
                }
                self.blacks[f.blacks].prepare_black(device, queue, *alpha as f32);
                draws.push(Draw::Black(f.blacks));
                f.blacks += 1;
            }
            Layer::Transition(_) => {
                unreachable!("prepare_layers sends transitions to Compositor::transition")
            }
            // Into its own slot, over its footage. A strict frame (export, headless) loads a shape it still
            // misses first, so it never draws the stand-in. For now every take is burned in: no frame has one yet.
            Layer::Cursor(c) => {
                let (target, strict) = (f.target, f.wait);
                let cursor = self.cursor_pass();
                if strict {
                    cursor.ensure(device, queue, c);
                }
                if let Some(range) = cursor.prepare(c, target, strict) {
                    draws.push(Draw::Cursor(range));
                }
            }
        }
    }
}
