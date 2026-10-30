//! A generator's size and frame rate (effect ABI v1.1): its definition's `resolution` and `fps`,
//! checked once when it is assembled. Both belong to generators; `fps` is 1–60.

use klipolo_model::contrib::{ContribDef, Resolution};

use super::ShaderKind;

/// The highest `fps` a generator may step at.
pub const MAX_STEP_FPS: u32 = 60;

/// Why a definition's `resolution` or `fps` can't be used: the diagnostic's code, what is wrong, and what to do.
pub(super) struct Refused {
    pub code: &'static str,
    pub message: String,
    pub hint: &'static str,
}

/// The definition's `resolution` (`Full` when unset) and `fps`, or why they are refused: a filter or transition sets
/// neither (a filter runs at half size through `downsample`), and `fps` is 1 to [`MAX_STEP_FPS`].
pub(super) fn timing(
    def: &ContribDef,
    kind: ShaderKind,
) -> Result<(Resolution, Option<u32>), Refused> {
    let generator = kind == ShaderKind::Generator;
    if def.resolution.is_some() && !generator {
        return Err(Refused {
            code: "rule.resolution",
            message: format!(
                "{}: only a generator chooses the size it draws at (`resolution`).",
                def.id
            ),
            hint: "Leave `resolution` out; a filter runs at half size through `downsample`.",
        });
    }
    match def.fps {
        Some(_) if !generator => Err(Refused {
            code: "rule.fps",
            message: format!("{}: only a generator steps its time (`fps`).", def.id),
            hint: "Leave `fps` out.",
        }),
        Some(fps) if !(1..=MAX_STEP_FPS).contains(&fps) => Err(Refused {
            code: "rule.fps",
            message: format!("{}: `fps` is 1 to {MAX_STEP_FPS}, not {fps}.", def.id),
            hint: "Give `fps` from 1 to 60, or leave it out to draw every frame.",
        }),
        fps => Ok((def.resolution.unwrap_or(Resolution::Full), fps)),
    }
}
