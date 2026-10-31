//! The Record dialog's live picture: what a take of the person's "What to record" would capture (`config::take_target`:
//! a display without Klipolo's own windows, or one app's windows on its display), in its own worker
//! (`--record-worker --live-view <target>`) at the small size (about 10 frames a second, at most 640 px on the long
//! side): no sound, nothing written. Whatever the page asks, it runs only while Screen Recording is allowed (read
//! without a prompt; no worker is launched otherwise) and never during a take; a new choice restarts it and the same one
//! keeps it; and a live picture whose worker ends by itself says so (`capture.liveView.unavailable`), so the page never
//! keeps its last frame as live. Its frames carry the choice's card key (`card_key`), so a frame of an earlier choice
//! still on its way is told apart.

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

/// The live picture of `what`. Its worker's arguments are the target as JSON alone: the small view.
pub(super) fn live_wanted(what: Option<&WhatToRecord>) -> Option<Wanted> {
    let what = what?;
    let target = serde_json::to_string(&take_target(what));
    let target = target
        .inspect_err(|error| tracing::warn!(%error, "recorder: the live picture's target"))
        .ok()?;
    Some(Wanted {
        what: target.clone(),
        key: card_key(what),
        args: Some(vec!["--record-worker".into(), "--live-view".into(), target]),
    })
}
