//! `setBackground`: one `SetBackground` with the project's background as the working copy has it, changed by what the
//! call gives; a value left null stays. A gradient given one colour fades it into itself until a second comes. A
//! background that comes out as it was adds no op. Whether a colour is one the project takes is the model's rule:
//! a call it refuses goes back to the model with the reason.

use klipolo_model::Project;
use klipolo_model::coded::ErrorParam;
use klipolo_model::ops::Op;
use klipolo_model::project::BackgroundKind;

use super::{Edit, label};
use crate::tools::BackgroundChoice;

/// The edit for `setBackground` on `work`.
pub(super) fn set_background(
    work: &Project,
    kind: Option<BackgroundChoice>,
    (color, color_end): (Option<&str>, Option<&str>),
    padding: Option<f64>,
) -> Edit {
    let mut background = work.background.clone();
    if let Some(kind) = kind {
        background.kind = kind.kind();
    }
    if let Some(color) = color {
        background.color = color.to_owned();
    }
    if let Some(end) = color_end {
        background.color_end = Some(end.to_owned());
    }
    if background.kind == BackgroundKind::Gradient && background.color_end.is_none() {
        background.color_end = Some(background.color.clone());
    }
    if let Some(padding) = padding {
        background.padding = padding;
    }
    let mut params = Vec::new();
    if let Some(kind) = kind {
        params.push(("kind", ErrorParam::from(kind.key())));
    }
    if let Some(padding) = padding {
        params.push(("padding", ErrorParam::Number(padding)));
    }
    Edit {
        ops: if background == work.background {
            Vec::new()
        } else {
            vec![Op::SetBackground { background }]
        },
        label: label("setBackground", params),
    }
}
