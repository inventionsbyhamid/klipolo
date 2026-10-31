//! `FakeBackend`: the backend tests use instead of the OS's. Its pipes run in real time on threads paced by
//! `klipolo_platform::clock::host_ns()`, send with `try_send` (counting drops) and deliver the `Script`'s events at their
//! times. Nothing here touches a device.
//!
//! Each pipe has a source key, which the `Script` names:
//! - `video`: "camera" for `Target::Camera`, else "screen"; its sound: "gameSound" for `Target::App`,
//!   `Target::Window` and a `Target::Portal` with an `app_id` (the game whose sound Linux links), else
//!   "computerSound";
//! - `audio`: "mic" for `AudioSpec::Mic`, "computerSound" for `AudioSpec::System`;
//! - `preview`: "camera";
//! - `live_view`: "liveView", whose first scripted event (any) ends its stream by itself at that time; its frames are
//!   its picture of `FAKE_DISPLAY` for the view it is told.
//! - `still`: its picture of `FAKE_DISPLAY` at the width asked, failing a target's first stills as `Script::still_fails`
//!   says; every still asked for is kept (`stills`).
//!
//! A `Target::Portal` pipe sends a fresh `PipeEvent::RestoreToken` ("fake-restore-1", "fake-restore-2", …) as it
//! starts, as the portal returns a new single-use token on every start.
//!
//! Every pipe of one backend keeps one schedule from the backend's creation: video frame `n` at `n / fps` and audio
//! frame 0 at the same instant, so flashes (every second) and blips (every second) line up across sources.

mod pipes;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use klipolo_platform::clock::host_ns;

use super::cursor::SyntheticCursor;
use super::fixture::FixtureVideo;
use super::packets::SyntheticVideoSpec;
use super::preview::{FAKE_DISPLAY, FakePreview};
use super::script::{Script, VideoFeed, video_spec};
use super::shell::FakeShell;
use crate::backend::{
    AudioSpec, Backend, CaptureCaps, Devices, EncoderResult, Permission, Permissions, PreviewFrame,
    PreviewPipe, ProbeReport, Target, VideoPipes, VideoSpec,
};
use crate::cursor::CursorSource;
use crate::media::Codec;
use crate::pace::PREVIEW_FPS;
use crate::pipe::{AudioPipe, PipeError, PipeEvent};
use crate::preflight::cursor::CursorPlan;
use crate::shell::Shell;

pub struct FakeBackend {
    script: Script,
    /// `VideoFeed::Fixture`'s packets, read once.
    fixture: Option<Arc<FixtureVideo>>,
    epoch_ns: u64,
    shell: Mutex<Option<FakeShell>>,
    /// Restore tokens handed out so far.
    tokens: AtomicU64,
    /// Pipes built (`video`, `audio`, `preview`, `live_view`): each is built to be started, so this counts the devices
    /// opened.
    opened: Arc<AtomicU64>,
    /// Pipe starts asked for (`VideoPipe::start`, `AudioPipe::start`): the device opens themselves.
    starts: Arc<AtomicU64>,
    /// Called as the first pipe is built (`on_first_build`).
    on_build: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Screen Recording requests (`request_screen_permission`).
    screen_requests: Arc<AtomicU64>,
    /// Each screen pipe start's cursor plan, in order.
    cursor_plans: Arc<Mutex<Vec<CursorPlan>>>,
    /// Each still asked for (`still`), in order: its target and width.
    stills: Arc<Mutex<Vec<(Target, u32)>>>,
}

impl FakeBackend {
    pub fn new(script: Script) -> Self {
        let fixture = match &script.video {
            VideoFeed::Fixture(path, _) => Some(Arc::new(FixtureVideo::load(path))),
            VideoFeed::Synthetic(_) => None,
        };
        Self {
            fixture,
            script,
            epoch_ns: host_ns(),
            shell: Mutex::new(None),
            tokens: AtomicU64::new(0),
            opened: Arc::default(),
            starts: Arc::default(),
            on_build: Mutex::new(None),
            screen_requests: Arc::default(),
            cursor_plans: Arc::default(),
            stills: Arc::default(),
        }
    }

    /// Each still asked for so far, in order: its target and width.
    pub fn stills(&self) -> Arc<Mutex<Vec<(Target, u32)>>> {
        self.stills.clone()
    }

    /// The cursor plan each screen pipe start had, in order (a heal's restart shows the cursor: Burned).
    pub fn cursor_plans(&self) -> Arc<Mutex<Vec<CursorPlan>>> {
        self.cursor_plans.clone()
    }

    /// How many pipe starts were asked for so far: each opens a device.
    pub fn starts(&self) -> Arc<AtomicU64> {
        self.starts.clone()
    }

    /// How many times Screen Recording was requested so far.
    pub fn screen_requests(&self) -> Arc<AtomicU64> {
        self.screen_requests.clone()
    }

    /// `hook` runs as the first pipe is built, before any starts: a test can send the app's Stop at that moment.
    pub fn on_first_build(self, hook: impl FnOnce() + Send + 'static) -> Self {
        *self.on_build.lock().expect("the build hook") = Some(Box::new(hook));
        self
    }

    fn built(&self) {
        self.opened.fetch_add(1, Ordering::Relaxed);
        let hook = self.on_build.lock().expect("the build hook").take();
        if let Some(hook) = hook {
            hook();
        }
    }

    /// How many pipes this backend has built so far, readable after the backend is handed over.
    pub fn opened(&self) -> Arc<AtomicU64> {
        self.opened.clone()
    }

    /// `shell()` hands out this shell (once); otherwise a shell with no inputs whose outputs nobody reads.
    pub fn with_shell(self, shell: FakeShell) -> Self {
        *self.shell.lock().expect("the fake shell") = Some(shell);
        self
    }
}

impl Backend for FakeBackend {
    fn caps(&self) -> CaptureCaps {
        self.script.caps.clone()
    }

    fn devices(&self) -> Devices {
        let mut devices = self.script.devices.clone();
        let gone: Vec<&String> = self.unplugged_now().map(|(d, _, _)| d).collect();
        devices.mics.retain(|m| !gone.contains(&&m.id));
        devices
    }

    fn permissions(&self) -> Permissions {
        Permissions {
            screen: self.script.screen_permission,
            camera: Permission::Granted,
            mic: Permission::Granted,
        }
    }

    fn video(&self, spec: &VideoSpec) -> Result<VideoPipes, PipeError> {
        self.built();
        let camera = matches!(spec.target, Target::Camera { .. });
        let key = if camera { "camera" } else { "screen" };
        let feed = match &self.script.video {
            VideoFeed::Synthetic(s) if camera => SyntheticVideoSpec {
                width: 1280,
                height: 720,
                ..s.clone()
            },
            VideoFeed::Synthetic(s) => s.clone(),
            VideoFeed::Fixture(_, fps) => SyntheticVideoSpec {
                fps: *fps,
                gop_frames: *fps,
                ..video_spec(320, 180)
            },
        };
        let cursor = if camera {
            CursorPlan::None
        } else {
            spec.cursor
        };
        let mut video = self.video_pipe(key, feed, cursor);
        video.preset = Some(spec.preset);
        video.encoders = (spec.encoders.iter())
            .map(|p| (p.clone(), self.script.refused_encoders.contains(&p.id)))
            .collect();
        if let Target::Portal { .. } = spec.target {
            let n = self.tokens.fetch_add(1, Ordering::Relaxed) + 1;
            let token = PipeEvent::RestoreToken(format!("fake-restore-{n}"));
            video.events.insert(0, (0, token));
        }
        let sound_key = match &spec.target {
            Target::App { .. }
            | Target::Window { .. }
            | Target::Portal {
                app_id: Some(_), ..
            } => "gameSound",
            _ => "computerSound",
        };
        let sound = spec
            .sound
            .then(|| Box::new(self.audio_pipe(sound_key, 2)) as Box<dyn AudioPipe>);
        Ok(VideoPipes {
            video: Box::new(video),
            sound,
        })
    }

    /// A `SyntheticCursor` when the script has one, else the screen's pipe sends the cursor.
    fn cursor_source(&self, _target: &Target) -> Result<Option<Box<dyn CursorSource>>, PipeError> {
        let source = |s| Box::new(SyntheticCursor::new(s)) as Box<dyn CursorSource>;
        Ok(self.script.cursor.clone().map(source))
    }

    fn audio(&self, spec: &AudioSpec) -> Result<Box<dyn AudioPipe>, PipeError> {
        self.built();
        Ok(match spec {
            AudioSpec::Mic { device } => Box::new(self.mic_pipe(device)),
            AudioSpec::System => Box::new(self.audio_pipe("computerSound", 2)),
        })
    }

    fn preview(&self, _camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        self.opened.fetch_add(1, Ordering::Relaxed);
        let fails = self.script.start_errors.get("camera").cloned();
        Ok(FakePreview::camera(fails, PREVIEW_FPS))
    }

    fn live_view(&self, _target: &Target) -> Result<Box<dyn PreviewPipe>, PipeError> {
        self.opened.fetch_add(1, Ordering::Relaxed);
        let fails = self.script.start_errors.get("liveView").cloned();
        let ends = self.events_of("liveView").first().map(|(ms, _)| *ms);
        Ok(FakePreview::live_view(
            fails,
            ends,
            self.script.live_view_refuses,
        ))
    }

    /// Its picture of `FAKE_DISPLAY` `width` wide (only a JPEG's start and end markers), unless the script fails this
    /// one of the target's stills.
    fn still(&self, target: &Target, width: u32) -> Result<PreviewFrame, PipeError> {
        let mut stills = self.stills.lock().unwrap_or_else(PoisonError::into_inner);
        let before = stills.iter().filter(|(t, _)| t == target).count();
        stills.push((target.clone(), width));
        let fails = (self.script.still_fails.iter())
            .find(|(t, _)| t == target)
            .map_or(0, |(_, n)| *n as usize);
        if before < fails {
            return Err(PipeError::Os("a fake still that didn't come".into()));
        }
        Ok(PreviewFrame {
            jpeg: vec![0xff, 0xd8, 0xff, 0xd9],
            width,
            height: width * FAKE_DISPLAY.1 / FAKE_DISPLAY.0,
        })
    }

    /// A computer with a dedicated hardware H.264 and HEVC encoder that both hold 1080p60, 1440p60 and 1080p120, and
    /// run 1440p at 120 fps flat out.
    fn probe(&self, _budget: Duration) -> ProbeReport {
        let encoder = |id: &str, codec| EncoderResult {
            id: id.into(),
            codec,
            hardware: true,
            dedicated: true,
            engine: id.into(),
            fps_1080p60: 60.0,
            fps_1440p60: 60.0,
            fps_1080p120: 120.0,
            max_fps_1440p: 120.0,
            latency_p95_ms: 8.0,
        };
        ProbeReport {
            encoders: vec![
                encoder("fake.h264", Codec::H264),
                encoder("fake.hevc", Codec::Hevc),
            ],
            took_ms: 0,
        }
    }

    /// Counted (`screen_requests`); nothing is asked.
    fn request_screen_permission(&self) {
        self.screen_requests.fetch_add(1, Ordering::SeqCst);
    }

    fn shell(&self) -> Box<dyn Shell> {
        let given = self.shell.lock().expect("the fake shell").take();
        Box::new(given.unwrap_or_else(|| FakeShell::new(vec![]).0))
    }
}
