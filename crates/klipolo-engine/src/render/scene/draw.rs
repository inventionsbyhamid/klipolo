//! A scene layer's frame (its shapes, paths and text): its instance found (or built), its
//! text found (or asked of the text build thread: the preview draws the latest it has meanwhile, an export waits),
//! its held target reused while the scene holds still, else evaluated and packed, its own target and its offscreen
//! nodes' sized to what they draw, then drawn (`draw_frame.rs`). An instance drawing a text entry it
//! hasn't drawn before takes that entry's units and lets its held target go.

use std::time::Instant;

use klipolo_model::scene::eval::{eval_into, static_until};

use super::backdrop::Backdrop;
use super::draw_frame::Frame;
use super::meshes::Source;
use super::said::{Short, say};
use super::sizes::{self, Packing, Parts};
use super::text_cache::TextAt;
use super::text_ensure::{self, Found, drawn_k};
use super::{Drawn, SceneAt, SceneDrawer, SceneOut, target};
use crate::plan::SceneLayer;
use crate::render::effects::{EffectStage, FrameInfo};

/// Whether a scene layer's own target may be only what the scene draws: nothing reads the layer's whole
/// target (no effect of its own, no rounded corners, no backdrop grabbed for it).
pub(super) fn shrinks(layer: &SceneLayer) -> bool {
    layer.effects.is_empty() && layer.radius <= 0.0 && !layer.backdrop
}

impl SceneDrawer {
    /// Draws `layer` at scene time `layer.t` for a target of `at.size` (`info.px` its pixels per 1080p pixel) and
    /// returns the target holding it and where that sits in one of `at.size` (only what the scene draws when nothing
    /// reads the whole: `shrinks`), or the target it holds when the scene hasn't changed since. `textures()` holds the
    /// layer's images'; its text is laid out as `text` says, on the text build thread, when its size, the fonts or its
    /// language changed, never per frame. None when the frame's budget has no target left (the stage said so).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        stage: &mut EffectStage,
        text: TextAt,
        layer: &SceneLayer,
        at: SceneAt,
        info: FrameInfo,
        backdrop: Option<Backdrop>,
        warnings: &mut Vec<String>,
    ) -> Option<SceneOut> {
        let started = Instant::now();
        // Its pixel sizes follow the layer's motion scale, as its text does: drawn at the text's
        // pixels per 1080p pixel, the layer's largest scale, the same every frame, so a pop scales the whole design and
        // its paths are tessellated once for it.
        let k = drawn_k(text, info);
        let i = match self.cache.find(layer, self.frame) {
            Some(i) => i,
            None => {
                self.stats.draw_builds += 1;
                self.build(device, queue, stage, text, layer, at, k as f32)
            }
        };
        let drawn = Drawn { size: at.size, k };
        let t = layer.t;
        let entry = self.cache.get_mut(i);
        entry.drawn = true;
        if entry.at != at {
            // Drawn at another size (a resize, another window or format): the same instance, a new target size; its
            // paths are fitted again below (made again only past a bucket) and its classes found again (they are per
            // size).
            entry.at = at;
            entry.sizes.reset();
            entry.survey_ask.again();
            if let Some(h) = entry.held.take() {
                stage.release_held(h.target);
            }
        }
        // A moving scene without text: its survey taken once it lands, or asked.
        self.survey_off(i, layer, (at, k), (device, queue));
        let entry = self.cache.get_mut(i);
        let mut src = Source {
            device,
            cache: &mut self.meshes,
            now: self.frame,
        };
        let found = match self.text.as_ref() {
            Some(tp) => {
                let gpu = (device, queue, tp);
                let redraw = self.redraw.as_ref();
                text_ensure::ensure(
                    &mut self.texts,
                    gpu,
                    layer,
                    entry,
                    at,
                    text,
                    redraw,
                    self.frame,
                )
            }
            None => Found::default(),
        };
        if let Some(k) = found.index {
            let laid = self.texts.get_mut(k);
            laid.drawn = true;
            if entry.text_id != laid.id {
                // Another text entry (it landed, the fonts changed, another size): its units, its survey when the build
                // thread made it (its own text's, for this size), and no held frame of the last one's glyphs.
                entry.text_id = laid.id;
                entry.inst.set_units_many(&laid.layouts.units);
                let built = laid
                    .survey
                    .as_ref()
                    .filter(|s| s.surveyed(at.size, laid.id));
                if let Some(built) = built.filter(|_| !found.stand_in) {
                    entry.sizes.adopt(built);
                    self.stats.survey_built_us = built.survey_us;
                }
                if let Some(h) = entry.held.take() {
                    stage.release_held(h.target);
                }
            }
        }
        if let Some(h) = entry.held {
            // Covered first: a hold that ends gives its target back before this frame takes one. A part of the layer
            // held while the layer since took an effect or corners (they read its whole target) isn't this frame
            //.
            let whole = h.rect == [0, 0, at.size.0, at.size.1];
            if (whole || shrinks(layer))
                && target::covers(&h, t, |from| static_until(&entry.inst, from))
                && let Some(target) = stage.held(h.target)
            {
                self.stats.held_frames += 1;
                self.stats.cpu_us_last = started.elapsed().as_secs_f64() * 1e6;
                return Some(SceneOut {
                    target,
                    rect: h.rect,
                });
            }
            stage.release_held(h.target);
            entry.held = None;
        }
        eval_into(&entry.inst, t, &mut entry.frame);
        entry
            .meshes
            .refresh(&mut src, &entry.inst, &entry.frame, &entry.statics, drawn);
        // Its pictures, bound; one still decoding is left out, and the frame isn't held.
        let ready = self
            .pictures
            .bind(device, self.pipeline.as_ref()?, &self.textures, self.frame);
        let text = found.index.map(|k| self.texts.get(k));
        let notes = text.map(|t| (t.layouts.notes, t.layouts.softer));
        let parts = Parts {
            inst: &entry.inst,
            statics: &entry.statics,
            spans: &entry.spans,
            drawn,
            pictures: &self.pictures.dims,
            meshes: &entry.meshes,
            text: text.map(|t| &t.layouts),
            backdrop: backdrop.map(|b| b.map),
        };
        // Its own target and its offscreen nodes' only as big as what they draw, the largest across its motion (Tasks
        // surveyed once, with its own text (one frame is all of a scene that holds one pose throughout).
        let packing = Packing {
            size: at.size,
            text: entry.text_id,
            shrink: shrinks(layer),
            survey: !found.stand_in,
            still: entry.still,
        };
        let surveys = entry.sizes.surveys;
        let (frame, packed) = (&mut entry.frame, &mut entry.packed);
        let root = sizes::pack(&parts, (frame, packed), &mut entry.sizes, t, packing);
        if entry.sizes.surveys > surveys {
            self.stats.surveys += 1;
            self.stats.survey_us = entry.sizes.survey_us;
        }
        let short = Short {
            over: entry.frame.counts().over(),
            detailed: entry.meshes.too_detailed,
            text: notes.map(|n| n.0).unwrap_or_default(),
            softer: notes.is_some_and(|n| n.1),
            text_missing: found.missing,
            groups: entry.packed.counts.too_many,
            reduced: entry.packed.reduced,
        };
        say(&mut self.said, &layer.contrib, short, warnings);
        let frame = Frame {
            entry: i,
            text: found.index,
            layer,
            root,
            info: FrameInfo {
                px: k as f32,
                content: FrameInfo::WHOLE,
                ..info
            },
            backdrop,
            ready,
            started,
        };
        self.draw_frame((device, queue, encoder), stage, frame, warnings)
    }
}
