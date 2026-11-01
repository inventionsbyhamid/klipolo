//! The backend for the first-run probe alone (`platform::backend_for_probe`): this OS's own probe, which
//! encodes generated frames only, and its caps; nothing else works on it (devices and permissions are empty, the pipes
//! and the preview Unsupported, the shell none), so having it without the consent guard can't capture.

use std::time::Duration;

use super::NoShell;
use crate::backend::{
    AudioSpec, Backend, CaptureCaps, DeviceInfo, Devices, Permissions, PreviewPipe, ProbeReport,
    Target, VideoPipes, VideoSpec,
};
use crate::pipe::{AudioPipe, PipeError};
use crate::shell::Shell;

pub(super) struct ProbeOnly<B>(pub(super) B);

impl<B: Backend> Backend for ProbeOnly<B> {
    fn caps(&self) -> CaptureCaps {
        self.0.caps()
    }
    fn devices(&self) -> Devices {
        Devices::default()
    }
    fn mics(&self) -> Vec<DeviceInfo> {
        vec![]
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
    /// Never a live view: this backend has no consent guard.
    fn live_view(&self, _target: &Target) -> Result<Box<dyn PreviewPipe>, PipeError> {
        Err(PipeError::Unsupported)
    }
    fn probe(&self, budget: Duration) -> ProbeReport {
        self.0.probe(budget)
    }
    /// Never the OS's request: this backend has no consent guard.
    fn request_screen_permission(&self) {}
    fn shell(&self) -> Box<dyn Shell> {
        Box::new(NoShell)
    }
}
