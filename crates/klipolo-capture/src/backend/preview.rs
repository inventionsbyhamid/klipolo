//! The previews' seam (Check camera's framing preview, the recorder's live view): a pipe that sends paced JPEG frames,
//! never written anywhere.

use std::sync::mpsc::SyncSender;

use crate::live_view_size::ViewPixels;
use crate::pipe::{PipeError, StopFlag};

/// A preview's frames (Check camera's, the live view's): never written anywhere. Paced by the pipe with
/// `pace::PreviewPacer` before each JPEG is made, so none is made to be dropped: Check camera's about 15 a second
/// (`pace::PREVIEW_FPS`), the live view's 10 in its small view, more in a big one (`live_view_size`). The worker sends
/// each one on.
pub trait PreviewPipe: Send {
    /// Starts the camera (or the live view's stream) and sends frames. It may wait on the system (macOS's camera
    /// prompt): `stopping` is the person's Cancel, checked right before it opens, so nothing opens once they cancelled.
    /// A pipe whose source stops sending by itself drops `frames`, so its worker hears the end.
    fn start(
        &mut self,
        frames: SyncSender<PreviewFrame>,
        stopping: StopFlag,
    ) -> Result<(), PipeError>;
    /// Idempotent.
    fn stop(&mut self);
    /// The live view's picture follows the view that shows it: before `start`, the view it starts for;
    /// after, a change applied to the running stream (`live_view_size`). Check camera's ignores it. False when the
    /// stream didn't take it (it didn't answer in time, or refused): its worker tries the same view again, so a stream
    /// is never left big while its view is small.
    fn resize(&mut self, _view: Option<ViewPixels>) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewFrame {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
