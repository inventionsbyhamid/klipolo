//! The camera's preview for the Record dialog's bubble, in a worker of its own (`--record-worker --check-camera`): the
//! camera alone, no mic and nothing written. It starts by itself only while the camera is allowed already; otherwise
//! only on the person's "Allow camera" press, when macOS asks (`Previews.ask`). A preview of the same camera is kept as
//! the dialog's other choices change.
//!
//! Every preview's close is enforced here (`closing`, the live view's, the stills' and the meter's too): the worker is
//! told to stop (it stops on Stop or its stdin's end) and kept until it ends, and killed when it hasn't
//! `CLOSE_WAIT_MS` later; one still there when a take arms or Klipolo exits is killed then, so none captures beside a
//! take or outlives Klipolo with the camera, the mic or the screen on. A closing preview's frames and events never reach the page.

use klipolo_capture::backend::Permission;
use klipolo_capture::protocol::Command;

use super::previews::Wanted;
use super::send::write_line;
use super::take::Worker;

/// How long a closed preview's worker has to end before it is killed.
pub(super) const CLOSE_WAIT_MS: u64 = 2_000;

/// A preview worker told to stop, until it ends.
pub(super) struct Closing {
    pub(super) worker: Worker,
    pub(super) by_ms: u64,
    pub(super) killed: bool,
}

/// The camera preview of `camera`, its worker's arguments only when it may start now: the camera is allowed, or the
/// person just pressed "Allow camera" (`asked`) and hasn't refused it before.
pub(super) fn camera_wanted(
    camera: Option<&String>,
    permission: Permission,
    asked: bool,
) -> Option<Wanted> {
    let camera = camera?;
    let may = may_open(permission, asked);
    let args = may.map(|_| {
        ["--record-worker", "--check-camera", camera]
            .map(String::from)
            .to_vec()
    });
    if args.is_none() {
        tracing::info!(
            ?permission,
            "recorder: the camera's preview waits for its permission or Allow camera"
        );
    }
    Some(Wanted {
        what: camera.clone(),
        key: camera.clone(),
        args,
        asking: may == Some(true),
    })
}

/// Whether a device's preview may open now, and whether it asks: `Some(false)` when allowed already, `Some(true)` on
/// the person's own press (`asked`) while macOS hasn't asked yet, None when refused or not asked for.
pub(super) fn may_open(permission: Permission, asked: bool) -> Option<bool> {
    match permission {
        Permission::Granted => Some(false),
        Permission::Denied => None,
        _ => asked.then_some(true),
    }
}

/// A preview's worker is told to stop, and kept until it ends, to be killed if that takes past `CLOSE_WAIT_MS`
/// (`enforce_close`).
pub(super) fn closing(list: &mut Vec<Closing>, mut worker: Worker, now_ms: u64) {
    write_line(&mut worker, &Command::Stop);
    list.push(Closing {
        worker,
        by_ms: now_ms + CLOSE_WAIT_MS,
        killed: false,
    });
}

/// Kills each closing preview worker still there past its wait (the ticker's).
pub(super) fn enforce_close(list: &mut [Closing], now_ms: u64) {
    for closing in (list.iter_mut()).filter(|c| !c.killed && now_ms >= c.by_ms) {
        tracing::warn!(
            generation = closing.worker.generation,
            "recorder: a preview didn't stop, so it is killed"
        );
        (closing.worker.kill)(true);
        closing.killed = true;
    }
}
