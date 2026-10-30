//! What the playback lookahead warms: the starts coming up within `LOOKAHEAD_US`, each planned and handed to `warm`
//! once, so decoders open (and images and LUTs load) before the frame that shows them. Going backward (shuttle) it
//! looks back instead: at what is entered from its end. Titles and motion graphics are warmed the same way, each with
//! a plan of its titles and scenes alone (built ahead on the title build thread
//! 1's note 2: a scene's instance and text built ahead the same way), so neither ever moves a decoder.

use klipolo_model::project::FormatKind;
use klipolo_model::time::Us;
use klipolo_model::timeline::{Item, ItemContent, Track};

use super::Playing;
use crate::plan::{FramePlan, Layer, PlanContext, frame_plan};
use crate::session::PreviewBackend;

/// A clip, overlay video or image starting this soon is handed to `warm`, so its decoder opens (or its image and
/// LUT load) before it's needed.
pub(super) const LOOKAHEAD_US: Us = 1_000_000;

/// Hands `backend` what the lookahead reaches from the frame at `at_us` that it hadn't yet, each once: the starts
/// ahead, or going backward what is entered from its end. `scratch` is reused frame to frame.
pub(super) fn warm_ahead<B: PreviewBackend>(
    ctx: &PlanContext,
    format: FormatKind,
    play: &mut Playing,
    at_us: Us,
    scratch: &mut Vec<Us>,
    backend: &mut B,
) {
    if play.rate < 0.0 {
        let until = at_us - LOOKAHEAD_US;
        if until < play.warmed_to {
            let before = play.warmed_to.min(at_us);
            upcoming_backward(ctx, until, before, scratch);
            play.warmed_to = until;
            for &at in scratch.iter() {
                backend.warm(&backward_plan(ctx, at, format));
            }
            titles_behind(ctx, until, before, scratch);
            for &at in scratch.iter() {
                backend.warm(&titles_only(frame_plan(ctx, at, format)));
            }
        }
    } else {
        let until = at_us + LOOKAHEAD_US;
        if until > play.warmed_to {
            let after = play.warmed_to.max(at_us);
            upcoming_starts(ctx, after, until, scratch);
            play.warmed_to = until;
            for &start in scratch.iter() {
                backend.warm(&frame_plan(ctx, start, format));
            }
            upcoming_titles(ctx, after, until, scratch);
            for &start in scratch.iter() {
                backend.warm(&titles_only(frame_plan(ctx, start, format)));
            }
        }
    }
}

/// `plan` with its titles and motion graphics alone: what is built ahead without a decoder (`FramePlan::titles_only`,
/// which the perf harness warms titles with, keeps the scenes too here).
pub(super) fn titles_only(mut plan: FramePlan) -> FramePlan {
    plan.layers
        .retain(|l| matches!(l, Layer::Text(_) | Layer::Scene(_)));
    plan
}

/// Appends the starts of visible titles (text items of the overlay tracks that show) in (`after`, `until`] to `out`,
/// sorted and without repeats. Binary searches only.
pub(super) fn upcoming_titles(ctx: &PlanContext, after: Us, until: Us, out: &mut Vec<Us>) {
    out.clear();
    for track in overlays(ctx) {
        let from = track.items.partition_point(|i| i.start_us <= after);
        let to = track.items.partition_point(|i| i.start_us <= until);
        out.extend(
            track.items[from..to]
                .iter()
                .filter(|i| is_title(i))
                .map(|i| i.start_us),
        );
    }
    out.sort_unstable();
    out.dedup();
}

/// Going backward: appends to `out`, sorted and without repeats, the last instant of each visible title that ends in
/// (`from`, `before`]: where it is entered from its end. Binary searches only.
pub(super) fn titles_behind(ctx: &PlanContext, from: Us, before: Us, out: &mut Vec<Us>) {
    out.clear();
    let end = |i: &Item| i.start_us + i.duration_us;
    for track in overlays(ctx) {
        let lo = track.items.partition_point(|i| end(i) <= from);
        let hi = track.items.partition_point(|i| end(i) <= before);
        out.extend(
            track.items[lo..hi]
                .iter()
                .filter(|i| is_title(i))
                .map(|i| end(i) - 1),
        );
    }
    out.sort_unstable();
    out.dedup();
}

/// A title, or a motion graphic: built ahead with a plan of its own kind alone (`titles_only`).
fn is_title(item: &Item) -> bool {
    match &item.content {
        ItemContent::Text { .. } => true,
        ItemContent::Generator { source, .. } => source.is_scene(),
        _ => false,
    }
}

/// Appends the starts of main clips and visible overlay videos and images in (`after`, `until`] to `out`, sorted and
/// without repeats. A clip whose cut has a transition first shows at its window's start, so that start is warmed
/// instead of the cut (warming the cut too would move the incoming decoder past the window's first frame); the plan
/// there says whether the outgoing clip starts with it too (`TransitionLayer::outgoing_starts`). A transition this
/// build doesn't have is a cut, so its cut is warmed and its window isn't. Binary searches only: nothing scans the
/// timeline per frame.
pub(super) fn upcoming_starts(ctx: &PlanContext, after: Us, until: Us, out: &mut Vec<Us>) {
    out.clear();
    let window = |starts: &[Us]| {
        let from = starts.partition_point(|&s| s <= after);
        let to = starts.partition_point(|&s| s <= until);
        from..to
    };
    let drawn = |at: Us| {
        ctx.transition_at(at)
            .filter(|w| ctx.transition_shader(w).is_some())
    };
    out.extend(
        ctx.starts[window(&ctx.starts)]
            .iter()
            .copied()
            .filter(|&start| drawn(start).is_none()),
    );
    let from = ctx.transitions.partition_point(|w| w.start_us <= after);
    let to = ctx.transitions.partition_point(|w| w.start_us <= until);
    out.extend(
        ctx.transitions[from..to]
            .iter()
            .filter(|w| ctx.transition_shader(w).is_some())
            .map(|w| w.start_us),
    );
    for track in overlays(ctx) {
        let from = track.items.partition_point(|i| i.start_us <= after);
        let to = track.items.partition_point(|i| i.start_us <= until);
        out.extend(
            track.items[from..to]
                .iter()
                .filter(|i| warmed(i))
                .map(|i| i.start_us),
        );
    }
    out.sort_unstable();
    out.dedup();
}

/// Overlay tracks that show (not hidden; soloed while any overlay track is). Their items are sorted by start and never
/// overlap, so their ends are sorted too.
fn overlays(ctx: &PlanContext) -> impl Iterator<Item = &Track> {
    let solo = ctx.solo;
    ctx.project
        .timeline
        .tracks
        .iter()
        .filter(move |t| solo.shows(t))
}

/// Overlay items the lookahead warms with the whole plan: videos and images. A motion graphic is built ahead of its
/// first frame with the titles' plan (`upcoming_titles`), which opens no decoder.
fn warmed(item: &Item) -> bool {
    matches!(
        item.content,
        ItemContent::Video { .. } | ItemContent::Image { .. }
    )
}

/// Going backward: appends to `out`, sorted and without repeats, the times in [`from`, `before`) where something is
/// entered from its end. That is the instant before each cut (the outgoing clip's last frame), a drawn transition's
/// last instant (`end_us - 1`, so its outgoing clip's decoder warms before the window is entered from its end), and
/// the last instant of each visible overlay video and image. A cut inside a drawn transition is left to its window,
/// which warmed the outgoing clip at its end. Binary searches only: nothing scans the timeline per frame.
pub(super) fn upcoming_backward(ctx: &PlanContext, from: Us, before: Us, out: &mut Vec<Us>) {
    out.clear();
    // An end `e` is entered at `e - 1`, which is in [from, before) when `e` is in (from, before].
    let drawn = |at: Us| {
        ctx.transition_at(at)
            .filter(|w| ctx.transition_shader(w).is_some())
    };
    let lo = ctx.starts.partition_point(|&s| s <= from);
    let hi = ctx.starts.partition_point(|&s| s <= before);
    out.extend(
        ctx.starts[lo..hi]
            .iter()
            .filter(|&&cut| cut > 0 && drawn(cut - 1).is_none())
            .map(|&cut| cut - 1),
    );
    let lo = ctx.transitions.partition_point(|w| w.end_us <= from);
    let hi = ctx.transitions.partition_point(|w| w.end_us <= before);
    out.extend(
        ctx.transitions[lo..hi]
            .iter()
            .filter(|w| ctx.transition_shader(w).is_some())
            .map(|w| w.end_us - 1),
    );
    let end = |i: &Item| i.start_us + i.duration_us;
    for track in overlays(ctx) {
        let lo = track.items.partition_point(|i| end(i) <= from);
        let hi = track.items.partition_point(|i| end(i) <= before);
        out.extend(
            track.items[lo..hi]
                .iter()
                .filter(|i| warmed(i))
                .map(|i| end(i) - 1),
        );
    }
    out.sort_unstable();
    out.dedup();
}

/// The plan to warm for `at_us` when it is entered backward. A transition's incoming clip is the one already playing
/// then, so only the outgoing side is warmed (warming the incoming one too would open it on the spare lane the
/// outgoing clip needs), unless the incoming clip ends with the window: nothing has played it yet, so both sides are.
/// `outgoing_starts` tells the pool to warm the outgoing side, as it does when a window opens on that clip's first frame.
pub(super) fn backward_plan(ctx: &PlanContext, at_us: Us, format: FormatKind) -> FramePlan {
    let mut plan = frame_plan(ctx, at_us, format);
    let incoming_ends = ctx.transition_at(at_us).is_some_and(|w| {
        w.end_us
            == ctx
                .starts
                .get(w.clip + 2)
                .copied()
                .unwrap_or(ctx.main_duration_us)
    });
    for layer in &mut plan.layers {
        if let Layer::Transition(t) = layer {
            t.outgoing_starts = true;
            if !incoming_ends {
                t.incoming.clear();
            }
        }
    }
    plan
}
