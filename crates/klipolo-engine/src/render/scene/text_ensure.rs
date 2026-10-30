//! The text a scene layer's frame draws: its own entry, landed or built
//! (an export waits for it), else the latest of the same text, a stand-in, while its own builds; and the pixels per
//! 1080p pixel it is laid out at.

use std::sync::Arc;

use tokio::sync::Notify;

use super::SceneAt;
use super::cache::Entry;
use super::text_cache::{Place, TextAt, TextCache, Upload};
use crate::plan::SceneLayer;

/// The text a frame draws (`ensure`): its entry, whether its own text can't be built, and whether the entry is a
/// stand-in (another size's, older fonts', an earlier value of its settings') while its own builds, or none is there.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Found {
    pub index: Option<usize>,
    pub missing: bool,
    pub stand_in: bool,
}

/// `layer`'s text drawn `at` as `text` says, for `entry`'s instance: kept, or landed and uploaded now, or (in the
/// preview, until its build lands) the latest of the same text, a stand-in; its room made in the entry's packing. None
/// while there is none, or when the instance has no text. With it, whether its own text can't be built (the build
/// thread is gone): what is drawn is an earlier text or none, and the frame says so (an export stops
/// on it).
#[allow(clippy::too_many_arguments)]
pub(super) fn ensure(
    texts: &mut TextCache,
    gpu: Upload,
    layer: &SceneLayer,
    entry: &mut Entry,
    at: SceneAt,
    text: TextAt,
    redraw: Option<&Arc<Notify>>,
    now: u64,
) -> Found {
    if !entry.has_text {
        return Found::default();
    }
    texts.collect(gpu, now);
    let place = Place {
        at,
        k: text.k.to_bits(),
        epoch: texts.epoch,
    };
    let (mut missing, mut stand_in) = (false, false);
    let found = match texts.find(layer, place, now) {
        Some(i) => Some(i),
        None => {
            let own = if text.block {
                texts
                    .request_waiting(layer, place, text.wait, redraw, gpu, now)
                    .and_then(|id| texts.wait_for(id, gpu, now))
            } else {
                texts.request(layer, place, text.wait, redraw, gpu);
                None
            };
            match own {
                Some(i) => Some(i),
                None => {
                    // An export's frame without its own text says so, whatever kept it from it.
                    missing = texts.gone || text.block;
                    stand_in = true;
                    texts.fallback(layer, at, now)
                }
            }
        }
    };
    let Some(i) = found else {
        // Nothing of its text yet: what it draws is no more its own than a stand-in's (not surveyed).
        return Found {
            index: None,
            missing,
            stand_in,
        };
    };
    let t = &texts.get(i).layouts;
    entry.packed.reserve_text((t.most, t.paints), t.nodes.len());
    Found {
        index: Some(i),
        missing,
        stand_in,
    }
}

/// The scene's target pixels per 1080p pixel: its text's (the frame's pixels × the layer's largest scale × the target's
/// cap), or the frame's own when that isn't a size.
pub(crate) fn drawn_k(text: TextAt, info: crate::render::effects::FrameInfo) -> f64 {
    if text.k.is_finite() && text.k > 0.0 {
        text.k
    } else {
        f64::from(info.px)
    }
}
