//! The Windows backend. It can't record yet; its tasks fill it in (compiled in CI, verified on a Windows device at W):
//! - the worker shell (notification-area item, HUD, `RegisterHotKey`, power, focus, the parent watch), the
//!   picker, game detection, the privacy preflight and mic listing through `IMMDeviceEnumerator`;
//! - Windows Graphics Capture, DXGI desktop duplication and NV12 on the GPU;
//! - the NVENC, AMF, QSV and Media Foundation encoders on D3D11 frames, and the probe;
//! - WASAPI process loopback for game sound, the system-loopback fallback and the Media Foundation camera.

use std::time::Duration;

use crate::backend::{
    AudioSpec, Backend, CaptureCaps, Devices, IndicatorPlace, Permissions, PickerKind, PreviewPipe,
    ProbeReport, Target, VideoPipes, VideoSpec,
};
use crate::pipe::{AudioPipe, PipeError};
use crate::shell::Shell;

pub(super) struct OsBackend;

impl OsBackend {
    pub(super) fn new() -> Self {
        Self
    }
}

impl Backend for OsBackend {
    fn caps(&self) -> CaptureCaps {
        CaptureCaps::none(IndicatorPlace::NotificationArea, PickerKind::OurList)
    }
    fn devices(&self) -> Devices {
        Devices::default()
    }
    fn permissions(&self) -> Permissions {
        Permissions::default()
    }
    fn video(&self, _spec: &VideoSpec) -> Result<VideoPipes, PipeError> {
        Err(PipeError::Unsupported)
    }
    fn audio(&self, _spec: &AudioSpec) -> Result<Box<dyn AudioPipe>, PipeError> {
        Err(PipeError::Unsupported)
    }
    fn preview(&self, _camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        Err(PipeError::Unsupported)
    }
    /// Not yet: Windows Graphics Capture's live view comes with its capture tasks (26-29). No view, no error.
    fn live_view(&self, _target: &Target) -> Result<Box<dyn PreviewPipe>, PipeError> {
        Err(PipeError::Unsupported)
    }
    fn probe(&self, _budget: Duration) -> ProbeReport {
        ProbeReport::default()
    }
    /// Windows has no screen-recording permission to ask for (its privacy pages cover the camera and the mic).
    fn request_screen_permission(&self) {}
    fn shell(&self) -> Box<dyn Shell> {
        Box::new(super::NoShell)
    }
}
