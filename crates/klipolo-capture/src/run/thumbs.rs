//! The Record dialog's cards: a still of each screen and app about once a second, in a worker of its own
//! (`worker --thumbs <targets>`), beside the chosen one's live view (`live_view`). No sound, nothing written, and no
//! orphan mode: it stops on the app's Stop or its stdin's end.
//!
//! - **Consent:** the guard first, so a test run stops there before any call to the backend; then Screen Recording,
//!   read without a prompt (`Backend::permissions`). When it isn't Granted, the worker refuses with
//!   `capture.permission.screen` and takes no still; each still reads the preflight again (`platform::macos::thumbs`).
//!   A still the system refuses (the person said no to macOS's "continue to allow" question, or took the permission
//!   back) ends the worker the same way, so the question is never raised again turn after turn; so does a target this
//!   computer can't take a still of (`Unsupported`).
//! - **The turns:** at most `MAX_THUMBS` targets, in the app's order, each once every `THUMB_EVERY`, their turns
//!   spread evenly over it, each still `THUMB_WIDTH` pixels wide in its target's shape. A still that doesn't come (a
//!   display gone, an app quit, a screenshot that timed out: `NoDevice`, `Os`) sends nothing and is tried again on its
//!   target's next turn. One that runs late moves the next turn back: stills never bunch up to catch up.
//! - Each still is a `Thumb` event under the app's key for its target.

use std::time::Duration;

use klipolo_model::coded::CodedError;

use super::preview_loop::{Events, PreviewIo, base64_of, the_app_s_word};
use crate::backend::{Backend, Permission, PreviewFrame, Target};
use crate::english::msg;
use crate::guard::assert_real_capture_allowed;
use crate::pipe::PipeError;
use crate::protocol::Event;

/// The most targets that get stills: the first ones the app names.
pub const MAX_THUMBS: usize = 8;
/// Each target's still comes this often.
pub const THUMB_EVERY: Duration = Duration::from_secs(1);
/// A still's width in pixels.
pub const THUMB_WIDTH: u32 = 320;

/// The guard first, then `thumbs_with`: the worker's form.
pub fn run_thumbs(backend: &dyn Backend, targets: &[(String, Target)], io: PreviewIo) -> i32 {
    assert_real_capture_allowed("the Record dialog's stills");
    thumbs_with(backend, targets, io)
}

/// Stills of `targets` (each with the app's key for it) to `io`'s output until the app's Stop or its stdin's end. 0 when
/// stopped (or given no target); 1 when Screen Recording isn't allowed or a still is refused (`Failed` says why).
pub fn thumbs_with(backend: &dyn Backend, targets: &[(String, Target)], io: PreviewIo) -> i32 {
    let PreviewIo {
        input,
        output,
        mut clock,
    } = io;
    let mut events = Events::new(output);
    let word = the_app_s_word(input);
    if backend.permissions().screen != Permission::Granted {
        tracing::info!("thumbs: screen recording isn't allowed, so no still is taken");
        events.send(&Event::Failed {
            error: msg!("capture.permission.screen"),
        });
        return 1;
    }
    if targets.len() > MAX_THUMBS {
        tracing::info!(
            targets = targets.len(),
            "thumbs: only the first {MAX_THUMBS} get stills"
        );
    }
    let targets = &targets[..targets.len().min(MAX_THUMBS)];
    if targets.is_empty() {
        return 0;
    }
    let turns = targets.len() as u32;
    // The current round's start and the turns taken in it; the next target's index.
    let (mut round, mut taken, mut next) = (clock.now(), 0, 0);
    // Which targets' stills are failing, so each run of failures is logged once.
    let mut failing = vec![false; targets.len()];
    while !word.stopped() {
        let (key, target) = &targets[next];
        match backend.still(target, THUMB_WIDTH) {
            Ok(still) => {
                failing[next] = false;
                events.send(&thumb(key, &still));
            }
            Err(error @ (PipeError::NoDevice | PipeError::Os(_))) => {
                if !std::mem::replace(&mut failing[next], true) {
                    tracing::info!(%error, %key, "thumbs: a still failed; it is tried again next turn");
                }
            }
            Err(error) => {
                tracing::info!(%error, %key, "thumbs: a still was refused, so the stills end");
                events.send(&Event::Failed {
                    error: refusal(&error),
                });
                return 1;
            }
        }
        next = (next + 1) % targets.len();
        taken += 1;
        if taken == turns {
            (round, taken) = (round + THUMB_EVERY, 0);
        }
        let mut due = round + THUMB_EVERY * taken / turns;
        let now = clock.now();
        if now > due {
            // Late: the next turn is now, and the round counts from it.
            (round, taken, due) = (now, 0, now);
        }
        if !clock.wait_until(due, &|| word.stopped()) {
            break;
        }
    }
    0
}

/// Why the stills ended, as the app hears it: a refused still is Screen Recording's step.
fn refusal(error: &PipeError) -> CodedError {
    match error {
        PipeError::PermissionDenied => msg!("capture.permission.screen"),
        _ => error.coded(),
    }
}

fn thumb(key: &str, still: &PreviewFrame) -> Event {
    Event::Thumb {
        key: key.to_owned(),
        jpeg_base64: base64_of(&still.jpeg),
        width: still.width,
        height: still.height,
    }
}
