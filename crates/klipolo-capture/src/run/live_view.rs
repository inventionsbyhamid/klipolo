//! The recorder's live view (the one capture before Record): a live picture of the take's target (the chosen
//! display, window or game) while the recorder is open, in its own worker (`worker --live-view <target>`), as Check
//! camera's preview is: no mic, no sound, nothing written, and no orphan mode (it stops on the app's Stop or its
//! stdin's end).
//!
//! - **Never a prompt:** Screen Recording is read first, without a prompt (`Backend::permissions`). When it isn't
//!   Granted, the view refuses with `capture.permission.screen` and calls nothing else; the backend's own pipe reads
//!   the preflight again before it lists anything (`platform::macos::live_view`).
//! - **Its frames:** in the recorder's small view, about 10 a second, at most 640 px on the long side; in its Large and
//!   Full screen views, the view's own pixel size within 1080p, at up to 30 a second (`live_view_size`).
//!   The view it starts for is the worker's last argument; the app's `ViewSize` changes it as the view changes size.
//!   Scaled by the system's capture itself and paced before each JPEG is made, each a `Preview` event (`preview_loop`).
//! - **Its end:** a stream that ends by itself says so (`capture.liveView.unavailable`), so the UI never keeps its last
//!   frame as live. A failed start says why: the permission's code, or the pipe's (`capture.error.unsupported` where
//!   this computer has no live view).

use std::io::{BufRead, Write};

use klipolo_model::coded::CodedError;

use super::preview_loop::{Ended, Events, run_preview, the_app_s_word};
use crate::backend::{Backend, Permission, Target};
use crate::english::msg;
use crate::live_view_size::ViewPixels;
use crate::pipe::PipeError;
use crate::protocol::Event;

/// The live view's frames for `view` (None: the small view) on stdout until the app's Stop or stdin's end.
pub fn run_live_view(backend: Box<dyn Backend>, target: &Target, view: Option<ViewPixels>) -> i32 {
    let input = Box::new(std::io::BufReader::new(std::io::stdin()));
    live_view_with(backend, target, view, input, Box::new(std::io::stdout()))
}

/// `run_live_view` with the app's lines from `input` and the events to `output`. 0 when stopped; 1 when it couldn't
/// start or its stream ended by itself (`Failed` says why).
pub fn live_view_with(
    backend: Box<dyn Backend>,
    target: &Target,
    view: Option<ViewPixels>,
    input: Box<dyn BufRead + Send>,
    output: Box<dyn Write + Send>,
) -> i32 {
    let mut events = Events::new(output);
    let word = the_app_s_word(input);
    let error = if backend.permissions().screen != Permission::Granted {
        tracing::info!("live view: screen recording isn't allowed, so nothing opens");
        msg!("capture.permission.screen")
    } else {
        // Built for the view it starts in.
        let open = || {
            let mut pipe = backend.live_view(target)?;
            pipe.resize(view);
            Ok(pipe)
        };
        match run_preview(open, &word, &mut events) {
            Ended::Stopped => return 0,
            Ended::Failed(e) => start_error(&e),
            Ended::Lost => msg!("capture.liveView.unavailable"),
        }
    };
    tracing::info!(code = %error.code, "live view: ended");
    events.send(&Event::Failed { error });
    1
}

/// Why the view didn't start, as the app hears it: a refused permission is Screen Recording's step.
fn start_error(error: &PipeError) -> CodedError {
    match error {
        PipeError::PermissionDenied => msg!("capture.permission.screen"),
        _ => error.coded(),
    }
}
