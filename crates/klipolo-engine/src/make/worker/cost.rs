//! A draft's `Cost` on this computer: measured (`measure`), then classed on time, memory and
//! CPU with its advice (`budget::classify`: the time class is the measurement's own, decided per point, and memory or
//! CPU can only raise it). A number costly at both ends is measured inside its range
//! static estimate (export keeps the static tiling).
//! `auto` is resolved, and the flags half size, cached and stepped are each on their own.

use klipolo_model::contrib::Resolution;

use super::compile::resolve_auto;
use crate::headless::Headless;
use crate::make::budget::{Cost, CostClass, MachineKey, budget_for, classify};
use crate::make::draft::DraftLayer;
use crate::make::envelope::costly_list;
use crate::make::measure::{Cold, Deadline, Meter, measure_noted};
use crate::make::protocol::MeasureSpec;
use crate::platform::gpu::GpuClass;

/// The draft's cost on this computer, and what `auto` resolved to.
pub(super) async fn cost_of(
    meter: &mut dyn Meter,
    h: &mut Headless,
    draft: &DraftLayer,
    spec: &MeasureSpec,
    deadline: Deadline,
) -> Result<(Cost, Option<Resolution>), String> {
    let facts = draft.facts();
    let costly = costly_list(
        &draft.def,
        facts.map_or(&[][..], |f| f.costly_params.as_slice()),
    );
    meter.begin();
    let info = h.device().adapter.get_info();
    let b = budget_for(GpuClass::of(&info));
    let (m, mut advice) = measure_noted(meter, h, draft, spec, &costly, deadline, &b).await?;
    let (class, runs_at, more) = classify(&m, facts, &draft.def, &b);
    advice.extend(more);
    advice.extend(cold_note(&meter.cold()));
    let resolved = match (draft.resolution(), class) {
        (Resolution::Auto, CostClass::Over) => None,
        (Resolution::Auto, CostClass::Heavy) => Some(Resolution::Half),
        (Resolution::Auto, _) => Some(resolve_auto(h, draft).await?),
        _ => None,
    };
    let machine = MachineKey::of(&info, &klipolo_platform::caps::current().os);
    let mut cost = Cost::of(&m, (class, runs_at, advice), &draft.def, &machine, &b);
    if cost.class != CostClass::Over {
        // What `auto` resolved to runs at that size.
        cost.resolved = resolved;
        cost.runs_at.half |= resolved == Some(Resolution::Half);
    }
    let resolved = cost.resolved;
    Ok((cost, resolved))
}

/// What the first frame at each size and scale cost while its pipelines compiled and its targets were made.
fn cold_note(cold: &[Cold]) -> Option<String> {
    let frames: Vec<String> = cold
        .iter()
        .map(|c| {
            let at = if c.half { " at half size" } else { "" };
            format!("{}×{}{at} {:.1} ms", c.size.0, c.size.1, c.ms)
        })
        .collect();
    (!frames.is_empty()).then(|| {
        format!(
            "First frames while the pipelines compiled (not counted): {}.",
            frames.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The half-size path's first frame is named as such, beside the full size's at the same size.
    #[test]
    fn the_cold_frames_name_their_scale() {
        let cold = [
            Cold {
                size: (1280, 720),
                half: false,
                ms: 202.7,
            },
            Cold {
                size: (1280, 720),
                half: true,
                ms: 13.1,
            },
        ];
        let note = cold_note(&cold).unwrap();
        assert!(
            note.contains("1280×720 202.7 ms, 1280×720 at half size 13.1 ms"),
            "{note}"
        );
    }
}
