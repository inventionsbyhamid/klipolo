//! Generator layers' pieces keep their place in the contribution's time (`timeline::piece_time`): what splits,
//! trims, new lengths and ripple cuts do to a piece's offset and span, and the `scenes@1` requirement. A layer that was
//! never split has no span, so its entrance stays at its start and its exit at its end whatever is trimmed.

use crate::project::Project;
use crate::time::Us;
use crate::timeline::ItemContent;

/// `Project::requires` while some generator layer shows a scene (an older build opens the file read-only).
pub const FEATURE: &str = "scenes@1";

/// One side of a split of the generator layer `whole_id`, `whole_us` long, starting `from_us` into it: both pieces keep
/// the span the layer had (its own length when it was never split), the second starting `from_us` further into it, and
/// the id the layer seeded its effects from (its own when it was never split), so random patterns continue.
pub(super) fn piece(content: &mut ItemContent, from_us: Us, whole_us: Us, whole_id: &str) {
    if let ItemContent::Generator {
        offset_us,
        span_us,
        seed_from,
        ..
    } = content
    {
        *span_us = Some(span_us.unwrap_or(whole_us));
        *offset_us = offset_us.saturating_add(from_us);
        seed_from.get_or_insert_with(|| whole_id.to_owned());
    }
}

/// A split piece's end moved by `delta_us` (its new length less its old, by a trim or a new length): its exit moves
/// with its end, so its span changes by as much.
pub(super) fn end_moved(content: &mut ItemContent, delta_us: Us) {
    if let ItemContent::Generator {
        span_us: Some(span),
        ..
    } = content
    {
        *span = span.saturating_add(delta_us);
    }
}

/// A split piece's start trimmed `removed_us` later (earlier when negative): what it shows stays where it was, so its
/// offset follows the start; a start before its span's beginning extends the span instead.
pub(super) fn start_moved(content: &mut ItemContent, removed_us: Us) {
    if let ItemContent::Generator {
        offset_us,
        span_us: Some(span),
        ..
    } = content
    {
        let at = offset_us.saturating_add(removed_us);
        if at >= 0 {
            *offset_us = at;
        } else {
            *offset_us = 0;
            *span = span.saturating_sub(at);
        }
    }
}

/// `cut_us` of a split piece removed by a ripple delete (a static layer's start and end map through the cut): it keeps
/// its entrance and exit, its span shorter by what went.
pub(super) fn cut(content: &mut ItemContent, cut_us: Us) {
    end_moved(content, cut_us.saturating_neg());
}

/// After every applied batch: `requires` holds [`FEATURE`] exactly while some generator layer's source is a scene
/// (`ContribId::is_scene`), as `transitions::normalize` keeps `transitions@1`, so a file whose last scene layer went
/// opens in builds without scenes again. O(layers); allocates only when it adds the requirement.
pub fn sync_scene_requirement(p: &mut Project) {
    let scenes = p.timeline.tracks.iter().flat_map(|t| &t.items).any(
        |item| matches!(&item.content, ItemContent::Generator { source, .. } if source.is_scene()),
    );
    if !scenes {
        p.requires.remove(FEATURE);
    } else if !p.requires.contains(FEATURE) {
        p.requires.insert(FEATURE.to_owned());
    }
}
