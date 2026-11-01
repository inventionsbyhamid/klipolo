//! Check camera's framing preview on the Mac: `MacPreviewPipe`, the `PreviewPipe` over the
//! same AVFoundation session the recording uses (`camera`), at the smallest format of at least 640×360, its output
//! scaled to fit 640×360 in the camera's shape and delivered as BGRA. About 15 frames a second (`pace::PreviewPacer`:
//! every other frame at 30 fps, every frame at 15, paced before encoding) become JPEG through Core Image (`jpeg`) and go
//! to Check camera with `try_send`: a frame is dropped, never waited for. It has no mic and writes nothing. Building it
//! opens nothing; `start` asks for the camera and opens it, the person's Stop checked before each
//! step; `stop` stops the session.
//!
//! Memory: one Core Image context, and a 640×360 JPEG of about 20-40 KB per frame in flight (at most the channel's two).

use std::sync::mpsc::SyncSender;

use cidre::cv;

use super::camera::{self, CameraSession, Delivered, OutputDelegateCallback, Want};
use super::host_time::host_ns_of;
use super::jpeg::Jpeg;
use crate::backend::{PreviewFrame, PreviewPipe};
use crate::guard::assert_real_capture_allowed;
use crate::pace::{PREVIEW_FPS, PreviewPacer};
use crate::pipe::{PipeError, StopFlag};
use crate::platform::common::camera::{FormatFacts, frame_durations, pick_format};
use crate::platform::common::sck::fit_size;

/// The preview's box; the camera's own shape is kept inside it.
const PREVIEW: (u32, u32) = (640, 360);
const FPS: u32 = 30;

pub struct MacPreviewPipe {
    /// The camera's unique id.
    id: String,
    session: Option<CameraSession>,
}

impl MacPreviewPipe {
    /// The guard first, then whether the camera is there: nothing is opened or asked for while it is built.
    pub(super) fn new(camera: &str) -> Result<Self, PipeError> {
        assert_real_capture_allowed("Check camera");
        if camera::device(camera).is_none() {
            return Err(PipeError::NoDevice);
        }
        Ok(Self {
            id: camera.to_owned(),
            session: None,
        })
    }
}

impl PreviewPipe for MacPreviewPipe {
    fn start(
        &mut self,
        frames: SyncSender<PreviewFrame>,
        stopping: StopFlag,
    ) -> Result<(), PipeError> {
        assert_real_capture_allowed("Check camera's camera");
        self.stop();
        camera::access(&stopping)?;
        let device = camera::device(&self.id).ok_or(PipeError::NoDevice)?;
        let formats = camera::formats(&device);
        let facts: Vec<FormatFacts> = formats.iter().map(|f| f.facts).collect();
        let i = pick_format(&facts, (PREVIEW.0, PREVIEW.1, FPS)).ok_or(PipeError::Unsupported)?;
        let size = fit_size(PREVIEW, (facts[i].width, facts[i].height));
        let want = Want {
            format: i,
            frame_durations: frame_durations(&formats[i].ranges, FPS),
            pixels: cv::PixelFormat::_32_BGRA,
            size: Some(size),
            preview: None,
        };
        let callback = feed(frames, Jpeg::new()?);
        if stopping.is_set() {
            return Err(PipeError::Os("stopped before the camera opened".into()));
        }
        let session = CameraSession::open(device, &want, callback, &stopping)?;
        tracing::info!(format = ?facts[i], ?size, "check camera: running");
        self.session = Some(session);
        Ok(())
    }

    fn stop(&mut self) {
        // Dropping the session stops it running (`stopRunning`), clears its delegate and removes its input and output.
        if self.session.take().is_some() {
            tracing::info!("check camera: stopped");
        }
    }
}

impl Drop for MacPreviewPipe {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Each buffer, on the session's queue: about 15 a second (`PreviewPacer`, by the frame's own time) become a JPEG,
/// offered to Check camera; the rest are never encoded.
fn feed(frames: SyncSender<PreviewFrame>, jpeg: Jpeg) -> OutputDelegateCallback {
    let mut pacer = PreviewPacer::at(PREVIEW_FPS);
    Box::new(move |delivered| {
        let Delivered::Frame(sample) = delivered else {
            return;
        };
        let Some(image) = sample.image_buf() else {
            return;
        };
        if !pacer.take(host_ns_of(sample.pts())) {
            return;
        }
        if let Some(bytes) = jpeg.encode(image) {
            let _ = frames.try_send(PreviewFrame {
                jpeg: bytes,
                width: image.width() as u32,
                height: image.height() as u32,
            });
        }
    })
}
