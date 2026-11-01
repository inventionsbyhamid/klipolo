//! The Record dialog's live picture: what a take of the person's "What to record" would capture (`config::take_target`:
//! a display without Klipolo's own windows, or one app's windows on its display), in its own worker
//! (`--record-worker --live-view <target> [<box>]`): no sound, nothing written. Its picture is made for the dialog's
//! box, in device pixels (`Previews.live_pixels`), so it is as sharp as the box shows it, within 1080p and this
//! computer's pixels a second (`live_view_size`); without a box, the small picture (about 10 frames a second, at most
//! 640 px on the long side). Whatever the page asks, it runs only while Screen Recording is allowed (read without a
//! prompt; no worker is launched otherwise) and never during a take; a new choice restarts it and the same one keeps
//! it; and a live picture whose worker ends by itself says so (`capture.liveView.unavailable`), so the page never
//! keeps its last frame as live. Its frames carry the choice's card key (`card_key`), so a frame of an earlier choice
//! still on its way is told apart.

use klipolo_capture::live_view_size::ViewPixels;

use super::config::take_target;
use super::previews::Wanted;
use super::types::WhatToRecord;

/// The card a choice is shown on, as a frame's `key` names it: `display:<display id>` or `app:<app id>`.
pub(super) fn card_key(what: &WhatToRecord) -> String {
    match what {
        WhatToRecord::Screen { display } => format!("display:{display}"),
        WhatToRecord::App { app_id, .. } => format!("app:{app_id}"),
    }
}

/// The live picture of `what` for a box of `pixels` (None: the small picture). Its worker's arguments are the target
/// as JSON, then the box's size as JSON when there is one.
pub(super) fn live_wanted(
    what: Option<&WhatToRecord>,
    pixels: Option<ViewPixels>,
) -> Option<Wanted> {
    let what = what?;
    let json = |value: serde_json::Result<String>| {
        value
            .inspect_err(|error| tracing::warn!(%error, "recorder: the live picture's arguments"))
            .ok()
    };
    let target = json(serde_json::to_string(&take_target(what)))?;
    let mut args = vec![
        "--record-worker".into(),
        "--live-view".into(),
        target.clone(),
    ];
    if let Some(pixels) = &pixels {
        args.push(json(serde_json::to_string(pixels))?);
    }
    Some(Wanted {
        what: target,
        key: card_key(what),
        args: Some(args),
        asking: false,
    })
}
