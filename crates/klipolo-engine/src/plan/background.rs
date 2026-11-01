//! The background under every frame.

use klipolo_model::project::{BackgroundKind, Project};

use super::BackgroundPlan;
use super::color::parse_hex;
use super::strings::Table;
use crate::registry::Registry;

/// Background gradient direction until the model has an angle; matches the DOM stage.
pub const GRADIENT_ANGLE_DEG: f64 = 135.0;

pub(super) fn background_plan(p: &Project, registry: &Registry, s: &mut Table) -> BackgroundPlan {
    let b = &p.background;
    let black = BackgroundPlan::Solid { color: [0, 0, 0] };
    match b.kind {
        // Behind the blurred footage (`plan::clip`), which covers it wherever a clip doesn't fill the frame.
        BackgroundKind::None | BackgroundKind::Blur => black,
        BackgroundKind::Solid => BackgroundPlan::Solid {
            color: parse_hex(&b.color),
        },
        BackgroundKind::Gradient => BackgroundPlan::Gradient {
            from: parse_hex(&b.color),
            to: parse_hex(b.color_end.as_deref().unwrap_or(&b.color)),
            angle_deg: GRADIENT_ANGLE_DEG,
        },
        BackgroundKind::Image => match b.asset_id.as_deref().and_then(|id| p.asset(id)) {
            Some(a) => BackgroundPlan::Image {
                asset_id: s.str(&a.id),
                path: s.path(&a.path, registry),
            },
            None => black,
        },
    }
}
