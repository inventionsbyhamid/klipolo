//! A scene's survey, and the one made on the text build thread (the timeline's pack kept off the frame that needs
//! it): when its text is laid out, or alone for a
//! scene without text that moves, an instance whose boxes need nothing only the render thread has
//! (path meshes, its pictures' sizes, the frame under a backdrop) is packed across its timeline there, as the render
//! thread would, and its classes travel with its text (or alone). The frame that takes them adopts them and packs
//! nothing more; a scene with offscreen nodes or a backdrop the build thread can't survey is surveyed on its first frame
//! as before, and one whose own target is all that is sized grows its class as it plays (`sizes.rs`).

use std::time::Instant;

use klipolo_model::scene::eval::{SceneFrame, eval_into, static_until};
use klipolo_model::scene::instance::SceneInstance;

use super::meshes::Meshes;
use super::pack::{Packed, Room};
use super::sizes::{Parts, Sizes};
use super::statics::{Look, NodeStatic};
use super::text_cache::Place;
use super::text_layouts::TextLayouts;
use super::{Drawn, SceneAt, SceneDrawer};
use crate::plan::SceneLayer;

/// Frames a second a survey packs, and the most it packs.
const SURVEY_FPS: f64 = 30.0;
const MAX_SURVEY: usize = 240;

/// Whether a scene's boxes need what only the render thread has: a path's mesh, a picture's size, a backdrop.
fn needs_render_thread(statics: &[NodeStatic]) -> bool {
    statics.iter().any(|s| {
        s.outline
            || matches!(
                s.look,
                Look::Path | Look::Polyline | Look::Image(_) | Look::Backdrop
            )
    })
}

/// A scene's survey asked of the text build thread alone: never (its text brings it, it holds one
/// pose, or its boxes need the render thread), not yet for its target's size, asked (its id), or answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SurveyAsk {
    Never,
    No,
    Asked(u64),
    Answered,
}

impl SurveyAsk {
    pub(super) fn of(has_text: bool, still: bool, statics: &[NodeStatic]) -> Self {
        if has_text || still || needs_render_thread(statics) {
            Self::Never
        } else {
            Self::No
        }
    }

    /// Its target changed size: asked again.
    pub(super) fn again(&mut self) {
        if *self != Self::Never {
            *self = Self::No;
        }
    }
}

impl SceneDrawer {
    /// Entry `i`'s survey asked alone (a moving scene without text drawn `at`, `k` target pixels per 1080p pixel):
    /// taken once it landed, its classes adopted when made for its target's size; asked when it isn't surveyed for
    /// that size and hasn't asked (from its build, so the lookahead's lands before its first frame).
    pub(super) fn survey_off(
        &mut self,
        i: usize,
        layer: &SceneLayer,
        (at, k): (SceneAt, f64),
        gpu: (&wgpu::Device, &wgpu::Queue),
    ) {
        let entry = self.cache.get_mut(i);
        match entry.survey_ask {
            SurveyAsk::Asked(id) => {
                let Some(survey) = self.texts.take_survey(id) else {
                    return;
                };
                entry.survey_ask = SurveyAsk::Answered;
                // A scene the render thread surveyed meanwhile (one with offscreen nodes, on its
                // first preview frame) keeps its classes: adopting again could shrink one grown since, and grow it once
                // more.
                let already = entry.sizes.surveyed(at.size, entry.text_id);
                if let Some(s) = survey.filter(|s| !already && s.surveyed(at.size, entry.text_id)) {
                    entry.sizes.adopt(&s);
                    self.stats.survey_built_us = s.survey_us;
                }
            }
            SurveyAsk::No if !entry.sizes.surveyed(at.size, entry.text_id) => {
                let place = Place {
                    at,
                    k: k.to_bits(),
                    epoch: 0,
                };
                let redraw = self.redraw.as_ref();
                if let Some(id) = self.texts.request_survey(layer, place, redraw, gpu) {
                    entry.survey_ask = SurveyAsk::Asked(id);
                }
            }
            _ => {}
        }
    }
}

/// `inst`'s classes across its timeline with its text `text` (its entry `id`; none and 0 for a scene without text, as
/// its instance's `text_id`) where it is `drawn`: None for a scene that holds one pose (its first frame is its survey)
/// or whose boxes need the render thread.
pub(super) fn survey_built(
    inst: &SceneInstance,
    (statics, spans): (&[NodeStatic], &[i32]),
    drawn: Drawn,
    text: Option<&TextLayouts>,
    id: u64,
) -> Option<Sizes> {
    let duration = inst.compiled().duration;
    if needs_render_thread(statics) || static_until(inst, 0.0) >= duration {
        return None;
    }
    let started = Instant::now();
    let meshes = Meshes::default();
    let mut packed = Packed::with_room(Room::of(inst, statics, &meshes));
    if let Some(text) = text {
        packed.reserve_text((text.most, text.paints), text.nodes.len());
    }
    let mut frame = SceneFrame::for_instance(inst);
    let parts = Parts {
        inst,
        statics,
        spans,
        drawn,
        pictures: &[],
        meshes: &meshes,
        text,
        backdrop: None,
    };
    let mut sizes = Sizes::with_room(inst.len());
    survey(
        &parts,
        &mut frame,
        &mut packed,
        &mut sizes,
        (drawn.size, id),
    );
    sizes.survey_us = started.elapsed().as_secs_f64() * 1e6;
    Some(sizes)
}

/// The survey: the instance's frames across its timeline evaluated into `frame` and packed into `packed` (their room
/// kept: no allocation), each offscreen node's box and the scene's own noted in `sizes`, for a target of `size` with
/// text `text`. The caller evaluates and packs its own frame again after (or, on the text build thread, hands the
/// classes over: `survey_built`).
pub(super) fn survey(
    parts: &Parts,
    frame: &mut SceneFrame,
    packed: &mut Packed,
    sizes: &mut Sizes,
    (size, text): ((u32, u32), u64),
) {
    if sizes.surveyed_for().is_some_and(|s| s != size) {
        sizes.reset();
    }
    let duration = parts.inst.compiled().duration;
    let duration = if duration.is_finite() {
        duration.max(0.0)
    } else {
        0.0
    };
    let n = ((duration * SURVEY_FPS).ceil() as usize).clamp(1, MAX_SURVEY);
    for i in 0..=n {
        eval_into(parts.inst, duration * i as f64 / n as f64, frame);
        packed.pack(&parts.with(frame));
        sizes.note(packed);
    }
    sizes.mark_surveyed(size, text);
}
