//! A transition's frame: the outgoing clip past its out point and the incoming clip before its in point, each
//! planned like a normal frame of its clip (`clip_layers`), joined by the transition's shader. A transition this build
//! doesn't have draws as a cut and is listed in `missing`.

use klipolo_model::project::FormatKind;
use klipolo_model::query::source_duration_us;
use klipolo_model::time::{Us, us_to_secs};
use klipolo_model::transitions::TransitionWindow;

use super::effects::{clamped, note_checking, note_not_drawn, pack_params, seed_of};
use super::strings::Table;
use super::{EffectPass, MissingContrib, PlanContext, TransitionLayer, clip_layers};
use klipolo_model::contrib::ContribKind;

/// The transition `w` at `at_us` (inside its window), or None when its shader isn't installed (the frame is then a
/// cut: the caller plans the clip under the playhead).
pub(super) fn transition_layer(
    ctx: &PlanContext,
    w: &TransitionWindow,
    at_us: Us,
    format: FormatKind,
    s: &mut Table,
    missing: &mut Vec<MissingContrib>,
) -> Option<TransitionLayer> {
    let p = &*ctx.project;
    let (out_i, in_i) = (w.clip, w.clip + 1);
    let t = p.timeline.main[out_i].transition_out.as_ref()?;
    let Some(shader) = ctx.transition_shader(w) else {
        note_not_drawn(
            &ctx.registry,
            missing,
            ContribKind::Transition,
            &t.effect,
            s,
        );
        return None;
    };
    // Within what this computer measured: the stored settings stay as typed.
    let params = clamped(&t.params, &shader.params, shader.envelope.as_deref());
    if let std::borrow::Cow::Owned(_) = params {
        note_checking(missing, ContribKind::Transition, s.str(t.effect.as_str()));
    }
    // Each side may show any of its media, not only its clip's part: the handles were rounded to timeline µs, so at
    // speeds other than 1× they can reach a few µs outside it, which the clamp in `clip_layers` keeps out.
    let limit = |i: usize| {
        let c = &p.timeline.main[i];
        source_duration_us(p, &c.source)
            .unwrap_or(c.source_out_us)
            .saturating_sub(1)
    };
    let mut outgoing = Vec::new();
    clip_layers(
        ctx,
        out_i,
        at_us - ctx.starts[out_i],
        limit(out_i),
        format,
        s,
        missing,
        &mut outgoing,
    );
    let mut incoming = Vec::new();
    clip_layers(
        ctx,
        in_i,
        at_us - w.cut_us,
        limit(in_i),
        format,
        s,
        missing,
        &mut incoming,
    );
    let span = (w.end_us - w.start_us).max(1);
    let into = at_us - w.start_us;
    let id = &p.timeline.main[out_i].id;
    Some(TransitionLayer {
        id: s.str(id),
        outgoing,
        incoming,
        // Only after a plain cut can the window open on the outgoing clip's first frame (a transition into it ends
        // inside it).
        outgoing_starts: ctx.starts[out_i] == w.start_us,
        // A transition fills the frame: it never grows (no margin).
        pass: EffectPass {
            time_s: us_to_secs(into) as f32,
            progress: (into as f64 / span as f64).clamp(0.0, 1.0) as f32,
            duration_s: us_to_secs(span) as f32,
            fps: p.settings.fps as f32,
            seed: seed_of(id, "transition"),
            ..EffectPass::host(shader.clone(), pack_params(shader, &params))
        },
    })
}
