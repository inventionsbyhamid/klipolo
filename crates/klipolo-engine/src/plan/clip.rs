//! One main clip's layers at an instant (`clip_layers`): its footage and, for recordings, the cursor and the camera,
//! framed for the format, with its look, the zoom, its effects and its fade. A normal frame plans one clip; a
//! transition plans two (`transition.rs`), so a frame can hold two cursors.

use klipolo_model::project::{Asset, FormatKind};
use klipolo_model::time::{Us, unscaled};

use super::framing::as_shown;
use super::strings::Table;
use super::{
    Layer, MediaMap, MissingContrib, PlanContext, SourceRef, VideoRole, cursor, effects, zoom,
};
use crate::registry::Registry;

mod footage;

use footage::main_layers;
pub use footage::{fill_crop, follow_crop_for, split_seam};

/// Main clip `i` at `local_us` into it, appended to `out`: its footage and camera (`main_layers`) with its look, the
/// zoom and its effects, then its fade to black. Inside a transition `local_us` runs past the clip's ends (negative
/// before its in point): the source time is clamped to `0..=source_limit` (the last µs it may show), and effects and
/// fades hold their value at the clip's edge.
#[allow(clippy::too_many_arguments)]
pub(super) fn clip_layers(
    ctx: &PlanContext,
    i: usize,
    local_us: Us,
    source_limit: Us,
    format: FormatKind,
    s: &mut Table,
    missing: &mut Vec<MissingContrib>,
    out: &mut Vec<Layer>,
) {
    let p = &*ctx.project;
    let registry = &*ctx.registry;
    let clip = &p.timeline.main[i];
    // `min` then `max`, not `clamp`: a limit below 0 (media of unknown length) mustn't panic. A freeze frame holds its
    // frame, inside a transition too.
    let source_us = match clip.freeze_at_us {
        Some(frame) => frame.max(0),
        None => (clip.source_in_us + unscaled(local_us, clip.speed))
            .min(source_limit)
            .max(0),
    };
    // The zoom follows the timeline: both sides of a transition take this instant's.
    let zoom = ctx.zoom.window(ctx.starts[i] + local_us);
    let local_us = local_us.clamp(0, clip.duration_us());
    let from = out.len();
    let read = (&*ctx.media, &*ctx.follows);
    main_layers(p, read, registry, clip, source_us, format, s, out);
    // Looks reach the main footage and the camera; zoom and the clip's effects only the main footage.
    let look = effects::look_plan(p, registry, clip, s, missing);
    let clock = effects::Clock {
        local_us,
        duration_us: clip.duration_us(),
        fps: p.settings.fps,
    };
    let mut clip_effects = Some(effects::effect_passes(
        registry,
        &clip.id,
        &clip.effects,
        clock,
        s,
        missing,
    ));
    let mut main = None;
    for (k, layer) in out.iter_mut().enumerate().skip(from) {
        if let Layer::Video(v) = layer {
            v.look = look.clone();
            if v.role == VideoRole::Main {
                // A zoom's target is where the person sees it: on mirrored footage, zoomed as shown, then put back.
                if let Some(window) = zoom {
                    v.crop = as_shown(
                        v.mirror,
                        zoom::zoom_crop(as_shown(v.mirror, v.crop), window),
                    );
                }
                v.effects = clip_effects.take().unwrap_or_default();
                v.pad_px = effects::pad_of(&v.effects);
                main = Some(k);
            }
        }
    }
    if let Some(k) = main {
        cursor::put_over(ctx, clip, source_us, format, s, out, k);
    }
    // Clip fades go to black over the footage, under the overlays.
    let alpha = effects::fade_alpha(clip, local_us);
    if alpha > 0.0 {
        out.push(Layer::Black { alpha });
    }
}

/// `asset` at `time_us` (clamped to its length), read from its editing copy when `media` has one (the preview's): the
/// copy's path and rate, no turn (copies are upright), the same times.
pub(super) fn source_at(
    asset: &Asset,
    time_us: Us,
    media: &MediaMap,
    registry: &Registry,
    s: &mut Table,
) -> SourceRef {
    let original = s.path(&asset.path, registry);
    // An empty map (export, and the preview without copies) returns before hashing.
    let (path, fps, rotation) = match media.get(asset.path.as_str()) {
        Some(copy) => (copy.path.clone(), copy.fps, Some(0)),
        None => (original.clone(), asset.fps, asset.rotation),
    };
    SourceRef {
        asset_id: s.str(&asset.id),
        path,
        original,
        time_us: time_us.clamp(0, (asset.duration_us - 1).max(0)),
        fps,
        rotation,
        width: asset.width,
        height: asset.height,
    }
}
