//! Frame presenters: how a rendered frame reaches the screen.
//! - `SocketPresenter`: the frame is read back to the CPU and sent over the loopback websocket into the webview's
//!   WebGL canvas. The fallback: dev, the fake engine and Playwright, a native start that failed, and the hidden
//!   "Compatibility preview" setting.
//! - `NativeLayerPresenter` (macOS): the frame is drawn straight into a CAMetalLayer under the transparent webview,
//!   so nothing comes back to the CPU.

pub mod native;
pub mod pick;
pub mod socket;

#[cfg(target_os = "macos")]
pub mod macos;

use crate::frames::buffer::FrameBytes;
use crate::frames::trailer::FrameMeta;
pub use native::{NativeFrame, NativeLayerPresenter, PresentLog, frames_dropped};
pub use socket::SocketPresenter;

/// What the backend makes of a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Output {
    /// RGBA pixels read back to the CPU (stills on the socket, export, tests).
    #[default]
    Rgba,
    /// NV12 pixels (BT.709) read back to the CPU: 1.5 bytes a pixel instead of 4 (playback on the socket).
    Nv12,
    /// Drawn into a native presenter's slot texture; nothing is read back.
    Native,
}

/// A finished frame, ready to show.
pub enum Frame {
    Bytes(FrameBytes),
    Native(NativeFrame),
}

/// Which presenter shows the frames (the UI draws socket frames itself, and only measures a native one's rect).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenterKind {
    Socket,
    Native,
}

/// Shows rendered frames. The actor owns one; it starts as the socket and becomes native when the app attaches a
/// layer (and goes back to the socket if the native one fails).
pub trait FramePresenter: Send {
    fn kind(&self) -> PresenterKind;
    /// What a frame should be rendered as (`playback`: a playback frame rather than a still).
    fn output(&self, playback: bool) -> Output;
    /// Shows `frame` (its pixels are on the GPU or in memory already) when `meta.due_ms` comes.
    fn present(&mut self, frame: Frame, meta: FrameMeta);
    /// The window was hidden or shown: a hidden presenter shows nothing and lets its drawables go.
    fn set_visible(&mut self, _visible: bool) {}
    /// No project is open: let go of GPU memory held for frames (the next frame makes it again).
    fn release(&mut self) {}
    /// A problem it can't recover from: the actor falls back to the socket.
    fn failed(&self) -> bool {
        false
    }
}
