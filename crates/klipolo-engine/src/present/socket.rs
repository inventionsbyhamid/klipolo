//! The websocket presenter: frames go to the webview over the loopback frame server, which draws them in
//! WebGL at their due time (PTS-based selection in `requestAnimationFrame`).

use super::{Frame, FramePresenter, Output, PresenterKind};
use crate::frames::server::FrameSink;
use crate::frames::trailer::{FrameMeta, append_trailer, now_ms};

pub struct SocketPresenter {
    sink: FrameSink,
    /// Playback frames go as NV12 (1.5 bytes a pixel); stills stay RGBA, so a paused frame is exact.
    nv12_playback: bool,
}

impl SocketPresenter {
    pub fn new(sink: FrameSink) -> Self {
        Self {
            sink,
            nv12_playback: true,
        }
    }

    /// Every frame as RGBA (the trial's "before" baseline).
    pub fn rgba_only(mut self) -> Self {
        self.nv12_playback = false;
        self
    }
}

impl FramePresenter for SocketPresenter {
    fn kind(&self) -> PresenterKind {
        PresenterKind::Socket
    }

    fn output(&self, playback: bool) -> Output {
        if playback && self.nv12_playback {
            Output::Nv12
        } else {
            Output::Rgba
        }
    }

    fn present(&mut self, frame: Frame, mut meta: FrameMeta) {
        let Frame::Bytes(mut bytes) = frame else {
            return;
        };
        meta.sent_ms = now_ms();
        append_trailer(bytes.vec_mut(), &meta);
        self.sink.publish(bytes.into_bytes());
    }
}
