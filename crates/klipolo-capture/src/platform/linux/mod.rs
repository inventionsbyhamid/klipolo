//! The Linux backend. It can't record yet (compiled and fake-tested in CI). What it needs:
//! - the worker shell (StatusNotifierItem tray, GlobalShortcuts portal or X11 keys, power, focus, the parent
//!   watch), game detection, preflight and mic listing from the PipeWire registry;
//! - the ScreenCast portal and PipeWire video, and the X11 fallback;
//! - the VA-API, NVENC and Vulkan encoders, the PipeWire mic and game sound, and the V4L2 and portal camera.

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
        // The portal's own picker chooses what to record.
        CaptureCaps::none(IndicatorPlace::Tray, PickerKind::System)
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
    /// None: the ScreenCast portal would ask at every opening of the recorder.
    fn live_view(&self, _target: &Target) -> Result<Box<dyn PreviewPipe>, PipeError> {
        Err(PipeError::Unsupported)
    }
    fn probe(&self, _budget: Duration) -> ProbeReport {
        ProbeReport::default()
    }
    /// Nothing to ask ahead: the ScreenCast portal asks as each take starts.
    fn request_screen_permission(&self) {}
    fn shell(&self) -> Box<dyn Shell> {
        Box::new(super::NoShell)
    }
}
