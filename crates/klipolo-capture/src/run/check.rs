//! Check camera (UX decision 8): a framing preview of the camera alone, no mic, nothing written. It runs only on the
//! person's Check camera press (`worker --check-camera`), and stops on Cancel, Disarm or Stop, or when the app goes:
//! unlike a take, it has no orphan mode. Frames arrive as JPEG from the backend (nothing is re-encoded here), about 15 a
//! second as the preview pipe paces them (`pace`), and each goes to the app as a `Preview` event (`preview_loop`).

use std::io::{BufRead, Write};

use super::preview_loop::{Ended, Events, run_preview, state, the_app_s_word};
use crate::backend::Backend;
use crate::protocol::Event;
use crate::shell::WorkerState;

/// Preview frames on stdout until Cancel or stdin's end.
pub fn run_check_camera(backend: Box<dyn Backend>, camera: &str) -> i32 {
    let input = Box::new(std::io::BufReader::new(std::io::stdin()));
    check_camera_with(backend, camera, input, Box::new(std::io::stdout()))
}

/// `run_check_camera` with the app's lines from `input` and the events to `output`. 0 when stopped, 1 when the camera
/// didn't start (`Failed` says why).
pub fn check_camera_with(
    backend: Box<dyn Backend>,
    camera: &str,
    input: Box<dyn BufRead + Send>,
    output: Box<dyn Write + Send>,
) -> i32 {
    let mut events = Events::new(output);
    events.send(&state(WorkerState::CheckingCamera));
    let word = the_app_s_word(input);
    match run_preview(|| backend.preview(camera), &word, &mut events) {
        Ended::Failed(e) => {
            events.send(&Event::Failed { error: e.coded() });
            return 1;
        }
        // A camera that stopped sending ends Check camera as a Stop does.
        Ended::Stopped | Ended::Lost => {}
    }
    // Check camera is over: the worker is ready again.
    events.send(&state(WorkerState::Ready));
    0
}
