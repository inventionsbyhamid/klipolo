//! The smoke run's backend: the real one with a quiet shell (AppKit's loop with nothing on screen) unless `--shell`
//! asks for its own, and its cursor source stopped at the second `--kill-cursor-at` gives.

use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use klipolo_capture::backend::*;
use klipolo_capture::cursor::{CursorEvent, CursorSource};
use klipolo_capture::media::StreamInfo;
use klipolo_capture::pipe::{AudioPipe, PipeError};
use klipolo_capture::shell::{Shell, ShellIo, ShellOutput};
use klipolo_cursor::ClickSource;

/// The real backend with a shell that shows nothing (no menu-bar item, no HUD, no shortcuts) unless asked for the real
/// one (`true`: `--shell`), and the cursor source stopped at the second given (`--kill-cursor-at`).
pub struct Quiet(pub Box<dyn Backend>, pub Option<f64>, pub bool);

/// AppKit's loop as an accessory app with nothing on screen, until the session's Quit: the main queue and the mouse
/// monitor need it.
struct NoFace;

impl Shell for NoFace {
    fn run(self: Box<Self>, io: ShellIo) {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        let Some(mtm) = MainThreadMarker::new() else {
            while io.output.recv().is_ok_and(|out| out != ShellOutput::Quit) {}
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let output = io.output;
        std::thread::spawn(move || {
            while output.recv().is_ok_and(|out| out != ShellOutput::Quit) {}
            cidre::dispatch::Queue::main().async_mut(stop_app);
        });
        app.run();
    }
}

/// AppKit's loop stopped, woken with an event so `run` returns now.
fn stop_app() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.stop(None);
    let wake = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        objc2_foundation::NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        0,
        0,
        0,
    );
    if let Some(wake) = wake {
        app.postEvent_atStart(&wake, false);
    }
}

/// A cursor source stopped `at_s` seconds after it started: the take's watchdog then burns the cursor in.
struct Killed {
    inner: Arc<Mutex<Box<dyn CursorSource>>>,
    at_s: f64,
}

impl CursorSource for Killed {
    fn start(&mut self, out: SyncSender<CursorEvent>) -> Result<(), PipeError> {
        let started = self.lock().start(out);
        if started.is_ok() {
            let (inner, at) = (self.inner.clone(), Duration::from_secs_f64(self.at_s));
            std::thread::spawn(move || {
                std::thread::sleep(at);
                println!("cursor source stopped (--kill-cursor-at)");
                inner.lock().unwrap_or_else(PoisonError::into_inner).stop();
            });
        }
        started
    }
    fn set_paused(&mut self, paused: bool) {
        self.lock().set_paused(paused);
    }
    fn stop(&mut self) {
        self.lock().stop();
    }
    fn clicks(&self) -> ClickSource {
        self.lock().clicks()
    }
}

impl Killed {
    fn lock(&self) -> std::sync::MutexGuard<'_, Box<dyn CursorSource>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Backend for Quiet {
    fn caps(&self) -> CaptureCaps {
        self.0.caps()
    }
    fn devices(&self) -> Devices {
        self.0.devices()
    }
    fn mics(&self) -> Vec<DeviceInfo> {
        self.0.mics()
    }
    fn permissions(&self) -> Permissions {
        self.0.permissions()
    }
    fn video(&self, spec: &VideoSpec) -> Result<VideoPipes, PipeError> {
        self.0.video(spec)
    }
    fn audio(&self, spec: &AudioSpec) -> Result<Box<dyn AudioPipe>, PipeError> {
        self.0.audio(spec)
    }
    fn cursor_source(&self, target: &Target) -> Result<Option<Box<dyn CursorSource>>, PipeError> {
        let source = self.0.cursor_source(target)?;
        Ok(match (source, self.1) {
            (Some(inner), Some(at_s)) => Some(Box::new(Killed {
                inner: Arc::new(Mutex::new(inner)),
                at_s,
            })),
            (source, _) => source,
        })
    }
    fn preview(&self, camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        self.0.preview(camera)
    }
    fn probe(&self, budget: Duration) -> ProbeReport {
        self.0.probe(budget)
    }
    fn shell(&self) -> Box<dyn Shell> {
        if self.2 {
            return self.0.shell();
        }
        Box::new(NoFace)
    }
    fn display_awake(&self) -> bool {
        self.0.display_awake()
    }
    fn decodes(&self, info: &StreamInfo, keyframe: &[u8]) -> Option<bool> {
        self.0.decodes(info, keyframe)
    }
}
