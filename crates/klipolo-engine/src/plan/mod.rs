//! What to draw at one instant in one format. Pure (no GPU, no IO), so preview, export and the gizmo parity
//! fixture agree by construction.

mod background;
pub mod captions;
pub mod clip;
pub mod color;
mod context;
mod cursor;
pub mod drawn;
pub(crate) mod effects;
pub mod framing;
mod generator;
mod layers;
pub mod motion;
pub mod overlay;
mod pass;
pub mod scene;
pub mod strings;
pub mod takes;
mod transition;
pub mod zoom;

use klipolo_model::project::FormatKind;
use klipolo_model::time::{US_PER_SEC, Us};

pub use background::GRADIENT_ANGLE_DEG;
pub use captions::CaptionLayer;
use clip::clip_layers;
pub use clip::follow_crop_for;
pub use context::{MediaCopy, MediaMap, PlanContext};
pub use cursor::{
    ARROW_PTS, PRESS, RIPPLE_REACH, RIPPLE_US, SHUTTER, cursor_layer, cursor_layer_id, shapes_in,
};
pub use effects::{clamped, pack_params, padded_dest, seed_of};
pub use generator::generator_time;
pub use layers::{
    AdjustmentLayer, BackgroundPlan, CursorDraw, CursorLayer, CursorMask, FramePlan,
    GeneratorLayer, ImageLayer, Layer, LookPlan, LutRef, MissingContrib, Ripple, SourceRef,
    TextLayer, TextOwner, TransitionLayer, VideoLayer, VideoRole,
};
pub use pass::{EffectPass, MAX_PARAM_BYTES, ParamBlock};
pub use scene::{MAX_SCENE_FILLS, SceneFills, SceneLayer};

/// The rate `kl.frame` counts at, the grain clock: every layer's grain refreshes at 30 Hz whatever the project's frame
/// rate, so preview and export draw the same grain at the same time (a generator's piece frame too).
pub const GRAIN_FPS: u32 = 30;

/// Start of the last whole frame before `duration_us` (0 for an empty video). Mirrors `lastFrameUs` in TS.
pub fn last_frame_us(duration_us: Us, fps: u32) -> Us {
    let fps = i128::from(fps.max(1));
    let us = i128::from(US_PER_SEC);
    let frames = (i128::from(duration_us.max(0)) * fps + us - 1) / us;
    (((frames - 1).max(0) * us + fps / 2) / fps) as Us
}

/// Start of frame `n` at `fps`, rounded to the nearest µs like `last_frame_us` (frame 1 at 30 fps starts at 33 333).
pub fn frame_start_us(n: i64, fps: u32) -> Us {
    let fps = i128::from(fps.max(1));
    ((i128::from(n.max(0)) * i128::from(US_PER_SEC) + fps / 2) / fps) as Us
}

/// The frame showing at `t_us`: the last one whose (rounded) start is at or before it.
pub fn frame_index(t_us: Us, fps: u32) -> i64 {
    let fps = i128::from(fps.max(1));
    let x = (i128::from(t_us.max(0)) + 1) * fps - fps / 2;
    ((x - 1) / i128::from(US_PER_SEC)) as i64
}

/// The start of the frame that contains `t_us` at `fps`.
pub fn frame_time_us(t_us: Us, fps: u32) -> Us {
    frame_start_us(frame_index(t_us, fps), fps)
}

/// Selection id of a recording clip's camera, shared with the UI.
pub fn camera_layer_id(clip_id: &str) -> String {
    format!("camera:{clip_id}")
}

pub fn frame_plan(ctx: &PlanContext, at_us: Us, format: FormatKind) -> FramePlan {
    let p = &*ctx.project;
    let registry = &*ctx.registry;
    let mut strings = ctx.strings.lock();
    let s = &mut *strings;
    let mut layers = Vec::new();
    let mut missing = Vec::new();
    if let Some(w) = ctx.transition_at(at_us)
        && let Some(t) = transition::transition_layer(ctx, w, at_us, format, s, &mut missing)
    {
        layers.push(Layer::Transition(t));
    } else if let Some((i, start)) = ctx.clip_at(at_us) {
        let clip = &p.timeline.main[i];
        let limit = clip.source_out_us - 1;
        clip_layers(
            ctx,
            i,
            at_us - start,
            limit,
            format,
            s,
            &mut missing,
            &mut layers,
        );
    }
    overlay::overlay_layers(ctx, at_us, format, s, &mut missing, &mut layers);
    // Captions over everything else.
    layers.extend(
        captions::caption_layer(ctx, at_us, format, &mut missing)
            .map(|c| Layer::Caption(Box::new(c))),
    );
    FramePlan {
        format,
        canvas: format.size(),
        at_us,
        background: background::background_plan(p, registry, s),
        layers,
        missing,
    }
}
