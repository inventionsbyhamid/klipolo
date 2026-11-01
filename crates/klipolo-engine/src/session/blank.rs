//! Draws only the background colour, with no GPU. The actor tests use it, and so does the app in its tests and off
//! macOS.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_model::time::Us;

use tokio::sync::Notify;

use super::backend::{AudioTime, Pixels, PreviewBackend, RenderRequest, Rendered};
use crate::audio::plan::AudioPlan;
use crate::frames::buffer::FramePool;
use crate::plan::{BackgroundPlan, FramePlan, VideoRole};

#[derive(Clone, Default)]
pub struct BlankBackend {
    log: Option<Arc<Mutex<Vec<RenderRequest>>>>,
    resets: Arc<AtomicU32>,
    audio: Arc<Mutex<Vec<&'static str>>>,
    warmed: Arc<Mutex<Vec<(VideoRole, String)>>>,
    frames: FramePool,
    /// A sound device that fails as soon as it is asked to play (tests of the fallback).
    failing_audio: bool,
    sounding: bool,
    audio_wake: Arc<Notify>,
}

impl BlankBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// A backend that records every request and counts resets (tests).
    pub fn recording() -> (Self, Arc<Mutex<Vec<RenderRequest>>>, Arc<AtomicU32>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let backend = Self {
            log: Some(log.clone()),
            ..Self::default()
        };
        let resets = backend.resets.clone();
        (backend, log, resets)
    }

    /// Its sound device fails on Play (it never opens a real one).
    pub fn with_failing_audio(mut self) -> Self {
        self.failing_audio = true;
        self
    }

    /// "start" and "stop" for each `start_audio` / `stop_audio` (it never opens a device).
    pub fn audio_log(&self) -> Arc<Mutex<Vec<&'static str>>> {
        self.audio.clone()
    }

    /// The (role, asset id) of every source handed to `warm`.
    pub fn warm_log(&self) -> Arc<Mutex<Vec<(VideoRole, String)>>> {
        self.warmed.clone()
    }
}

impl PreviewBackend for BlankBackend {
    async fn render(&mut self, req: RenderRequest) -> Result<Rendered, String> {
        let [r, g, b] = match &req.plan.background {
            BackgroundPlan::Solid { color } | BackgroundPlan::Gradient { from: color, .. } => {
                *color
            }
            BackgroundPlan::Image { .. } => [0, 0, 0],
        };
        let (width, height) = (req.width, req.height);
        let mut frame = self.frames.take((width * height * 4) as usize);
        let bytes = frame.vec_mut();
        for _ in 0..width * height {
            bytes.extend_from_slice(&[r, g, b, 255]);
        }
        if let Some(log) = &self.log {
            log.lock().unwrap_or_else(PoisonError::into_inner).push(req);
        }
        Ok(Rendered {
            pixels: Pixels::Ready(Some(frame)),
            format: crate::frames::trailer::FORMAT_RGBA8,
            width,
            height,
            stride: width * 4,
            warnings: Vec::new(),
            missing: Vec::new(),
            not_answering: Vec::new(),
        })
    }

    fn reset(&mut self) {
        self.resets.fetch_add(1, Ordering::SeqCst);
    }

    fn start_audio(&mut self, _plan: Arc<AudioPlan>, _from_us: Us) -> bool {
        self.audio
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push("start");
        self.sounding = self.failing_audio;
        if self.sounding {
            self.audio_wake.notify_one();
        }
        self.sounding
    }

    fn audio_time(&self) -> AudioTime {
        if self.sounding {
            AudioTime::Failed("no output device".into())
        } else {
            AudioTime::Off
        }
    }

    fn audio_wake(&self) -> Option<Arc<Notify>> {
        Some(self.audio_wake.clone())
    }

    fn warm(&mut self, plan: &FramePlan) {
        self.warmed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(
                plan.video_sources()
                    .into_iter()
                    .map(|(role, s)| (role, s.asset_id.to_string())),
            );
    }

    fn stop_audio(&mut self) {
        self.sounding = false;
        self.audio
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push("stop");
    }
}
