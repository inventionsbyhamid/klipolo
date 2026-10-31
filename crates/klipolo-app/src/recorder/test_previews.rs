//! The preview tests' world: the recorder's fake world (fake workers, the clock the test moves) on a fake computer with
//! one display, one app with windows, a camera and a mic, whose Screen Recording, camera and mic permissions the test
//! sets and changes as it runs (read again at every ask), and a page that keeps the frames and events it gets. The
//! dialog's setup is read first, as the dialog does on opening. Nothing captures and nothing asks.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use klipolo_capture::backend::{
    AppInfo, AudioSpec, Backend, CaptureCaps, Permission, Permissions, PreviewPipe, ProbeReport,
    VideoPipes, VideoSpec,
};
use klipolo_capture::pipe::{AudioPipe, PipeError};
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::shell::Shell;
use klipolo_capture::synthetic::Script;
use klipolo_model::coded::CodedError;

use super::frames::PreviewOut;
use super::test_launch::Logged;
use super::test_support::*;
use super::types::{PreviewFrame, PreviewId, Previews, WhatToRecord};

/// The fake computer, with the permissions the test sets. It never asks for Screen Recording (only the Allow press's
/// command does, `permissions`).
struct Allowing {
    inner: Arc<dyn Backend>,
    now: Arc<Mutex<Permissions>>,
}

impl Backend for Allowing {
    fn caps(&self) -> CaptureCaps {
        self.inner.caps()
    }
    fn devices(&self) -> klipolo_capture::backend::Devices {
        self.inner.devices()
    }
    fn permissions(&self) -> Permissions {
        *self.now.lock().unwrap()
    }
    fn mic_permission(&self) -> Permission {
        self.permissions().mic
    }
    fn video(&self, spec: &VideoSpec) -> Result<VideoPipes, PipeError> {
        self.inner.video(spec)
    }
    fn audio(&self, spec: &AudioSpec) -> Result<Box<dyn AudioPipe>, PipeError> {
        self.inner.audio(spec)
    }
    fn preview(&self, camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        self.inner.preview(camera)
    }
    fn probe(&self, budget: Duration) -> ProbeReport {
        self.inner.probe(budget)
    }
    fn shell(&self) -> Box<dyn Shell> {
        self.inner.shell()
    }
}

/// What the page got: each frame's header and JPEG, and each preview's events.
#[derive(Default)]
pub(super) struct Page {
    frames: Mutex<Vec<(PreviewFrame, Vec<u8>)>>,
    events: Mutex<Vec<(PreviewId, Event)>>,
}

impl PreviewOut for Page {
    fn frame(&self, message: Vec<u8>) -> bool {
        let length = usize::from(u16::from_be_bytes([message[0], message[1]]));
        let header = serde_json::from_slice(&message[2..2 + length]).expect("a frame's header");
        let jpeg = message[2 + length..].to_vec();
        self.frames.lock().unwrap().push((header, jpeg));
        true
    }

    fn event(&self, id: PreviewId, ev: &Event) {
        self.events.lock().unwrap().push((id, ev.clone()));
    }
}

/// A recorder whose dialog read its setup, its page, and the permissions the test changes.
pub(super) struct Previewing {
    pub(super) rec: Fixture,
    allowed: Arc<Mutex<Permissions>>,
    pub(super) page: Arc<Page>,
}

impl std::ops::Deref for Previewing {
    type Target = Fixture;
    fn deref(&self) -> &Fixture {
        &self.rec
    }
}

/// The app the fake computer runs with windows.
pub(super) const NOTES: &str = "com.apple.Notes";

/// A recorder on the fake computer with `allowed`, its dialog's setup read.
pub(super) fn previewing(allowed: Permissions) -> Previewing {
    let mut script = Script::default();
    script.devices.apps.push(AppInfo {
        app_id: NOTES.into(),
        name: "Notes".into(),
        pid: 501,
        is_game: false,
        frontmost: false,
        exe: None,
    });
    let allowed = Arc::new(Mutex::new(allowed));
    let now = allowed.clone();
    let rec = fixture_changed(script, move |parts| {
        let make = std::mem::replace(&mut parts.backend, Box::new(|| unreachable!()));
        let backend: Arc<dyn Backend> = Arc::new(Allowing { inner: make(), now });
        parts.backend = Box::new(move || backend.clone());
    });
    rec.state.setup().unwrap();
    Previewing {
        rec,
        allowed,
        page: Arc::default(),
    }
}

/// Screen Recording, the camera and the mic all allowed.
pub(super) fn all_allowed() -> Permissions {
    Permissions {
        screen: Permission::Granted,
        camera: Permission::Granted,
        mic: Permission::Granted,
    }
}

impl Previewing {
    /// The page asks for `want`.
    pub(super) fn set(&self, want: Previews) -> Result<(), CodedError> {
        self.settle();
        let set = self.state.previews(&want, self.page.clone());
        self.settle();
        set
    }

    /// The person changes a permission (in System Settings, or by answering macOS).
    pub(super) fn allow(&self, change: impl FnOnce(&mut Permissions)) {
        change(&mut self.allowed.lock().unwrap());
    }

    /// The clock moves `ms` milliseconds, and the previews' ticker ticks.
    pub(super) fn advance_ms(&self, ms: u64) {
        self.settle();
        self.clock.add_ms(ms);
        self.state.preview_workers().unwrap().tick();
        self.settle();
    }

    /// The latest worker launched with `flag` (`--thumbs`, `--live-view`, `--check-camera`, `--meter`).
    pub(super) fn worker(&self, flag: &str) -> usize {
        self.launch
            .worker_with(flag)
            .unwrap_or_else(|| panic!("a worker launched with {flag}"))
    }

    /// The arguments of every worker launched with `flag`, in order.
    pub(super) fn launched(&self, flag: &str) -> Vec<Vec<String>> {
        self.settle();
        (self.launch.log().into_iter())
            .filter_map(|l| match l {
                Logged::Spawn(args) if args.iter().any(|a| a == flag) => Some(args),
                _ => None,
            })
            .collect()
    }

    /// The commands worker `worker` was sent.
    pub(super) fn told(&self, worker: usize) -> Vec<Command> {
        self.settle();
        self.launch.commands_to(worker)
    }

    /// Worker `worker` says `ev`.
    pub(super) fn says(&self, worker: usize, ev: Event) {
        self.launch.emit_to(worker, ev);
        self.settle();
    }

    /// The frames the page got.
    pub(super) fn frames(&self) -> Vec<(PreviewFrame, Vec<u8>)> {
        self.settle();
        self.page.frames.lock().unwrap().clone()
    }

    /// The previews' events the page got.
    pub(super) fn heard(&self) -> Vec<(PreviewId, Event)> {
        self.settle();
        self.page.events.lock().unwrap().clone()
    }
}

/// The whole first display.
pub(super) fn screen() -> WhatToRecord {
    WhatToRecord::Screen {
        display: "display-1".into(),
    }
}

/// Notes' windows on the first display.
pub(super) fn notes() -> WhatToRecord {
    WhatToRecord::App {
        app_id: NOTES.into(),
        display: "display-1".into(),
    }
}

/// No preview at all.
pub(super) fn nothing() -> Previews {
    Previews {
        thumbs: false,
        live: None,
        camera: None,
        mic: None,
        ask: None,
    }
}

/// Every preview: the cards, the whole display live, the camera and the mic.
pub(super) fn everything() -> Previews {
    Previews {
        thumbs: true,
        live: Some(screen()),
        camera: Some("camera-1".into()),
        mic: Some("mic-1".into()),
        ask: None,
    }
}

/// A JPEG's bytes (its markers only).
pub(super) const JPEG: [u8; 6] = [0xff, 0xd8, 0x00, 0x7f, 0xff, 0xd9];

/// A live picture's or the camera's frame of `JPEG`.
pub(super) fn frame() -> Event {
    Event::Preview {
        jpeg_base64: STANDARD.encode(JPEG),
        width: 2,
        height: 2,
    }
}

/// A card's still of `JPEG`.
pub(super) fn still(key: &str) -> Event {
    Event::Thumb {
        key: key.into(),
        jpeg_base64: STANDARD.encode(JPEG),
        width: 2,
        height: 2,
    }
}

/// A preview's `Failed` with `code`.
pub(super) fn failed(code: &str) -> Event {
    Event::Failed {
        error: CodedError::new(code),
    }
}
