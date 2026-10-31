//! The Record dialog's mic meter in its worker, with the fake backend's synthetic sound: the loudest sample of each
//! 100 ms of the mic until the app's Stop; the mic opened only while it is allowed (or on the person's press), and the
//! meter's end when the mic goes or is refused; neither the meter nor the stills write a file; and in a test run both
//! stop at the consent guard before any call to the backend. Nothing captures.
#![cfg(feature = "synthetic")]

#[path = "common/preview_io.rs"]
#[allow(dead_code, reason = "the meter's app sends its lines at one time each")]
mod preview_io;

use std::io::{BufReader, Cursor};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use klipolo_capture::backend::{
    AudioSpec, Backend, CaptureCaps, DeviceInfo, Devices, Permission, Permissions, PreviewFrame,
    PreviewPipe, ProbeReport, Target, VideoPipes, VideoSpec,
};
use klipolo_capture::cursor::CursorSource;
use klipolo_capture::health::SILENCE_DBFS;
use klipolo_capture::media::StreamInfo;
use klipolo_capture::pipe::{AudioPipe, PipeError, PipeEvent};
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::run::PreviewIo;
use klipolo_capture::run::meter::{METER_EVERY, PERMISSION_EVERY, meter_with, run_meter};
use klipolo_capture::run::thumbs::{run_thumbs, thumbs_with};
use klipolo_capture::shell::Shell;
use klipolo_capture::synthetic::{FakeBackend, Script, Signal};
use preview_io::{SharedOut, sent_after};

/// The meter of "mic-1" on `backend` (`ask`: after the person's "Allow microphone" press), the app's Stop sent
/// `stop_ms` after it starts: its exit code and what it told the app.
fn meter_on(backend: &FakeBackend, ask: bool, stop_ms: u64) -> (i32, Vec<Event>) {
    let out = SharedOut::default();
    let io = PreviewIo::new(sent_after(&Command::Stop, stop_ms), Box::new(out.clone()));
    let code = meter_with(backend, "mic-1", ask, io);
    (code, out.events())
}

/// `meter_on` a new fake backend with `script`, the mic allowed.
fn meter(script: Script, stop_ms: u64) -> (i32, Vec<Event>) {
    meter_on(&FakeBackend::new(script), false, stop_ms)
}

/// Each `Meter` event's source and peak.
fn peaks(events: &[Event]) -> Vec<(&str, f32)> {
    (events.iter())
        .filter_map(|e| match e {
            Event::Meter { source, peak_dbfs } => Some((source.as_str(), *peak_dbfs)),
            _ => None,
        })
        .collect()
}

fn failed(events: &[Event]) -> Vec<&str> {
    (events.iter())
        .filter_map(|e| match e {
            Event::Failed { error } => Some(error.code.as_str()),
            _ => None,
        })
        .collect()
}

/// A sine at −6 dBFS, without the fake's sync blips.
fn sine() -> Script {
    Script {
        sound: Signal::Tone {
            hz: 440.0,
            dbfs: -6.0,
        },
        sync_blips: false,
        ..Script::default()
    }
}

#[test]
fn meter_sends_a_peak_every_100_ms() {
    assert_eq!(METER_EVERY, Duration::from_millis(100));
    let (code, events) = meter(sine(), 1_000);
    assert_eq!(code, 0);
    let peaks = peaks(&events);
    // Each peak is 100 ms of the mic's own time, so 1 s gives at most 10 (and, however busy the computer, a good part
    // of them); each is the mic's, at the sine's peak.
    assert!((5..=10).contains(&peaks.len()), "{} peaks", peaks.len());
    for (source, dbfs) in &peaks {
        assert_eq!(*source, "mic");
        assert!((dbfs + 6.0).abs() < 0.5, "{dbfs} dBFS");
    }
    assert!(failed(&events).is_empty(), "{events:?}");
}

#[test]
fn meter_sends_the_loudest_sample_of_each_100_ms() {
    // Silence, with the fake's sync blip (10 ms at −6 dBFS, one block) once a second: the 100 ms that hold a blip
    // show it, whichever of their blocks it is, and the rest show silence.
    let script = Script {
        sound: Signal::Zeros,
        ..Script::default()
    };
    let (code, events) = meter(script, 2_500);
    assert_eq!(code, 0);
    let peaks = peaks(&events);
    let blips = (peaks.iter())
        .filter(|(_, dbfs)| (dbfs + 6.0).abs() < 0.5)
        .count();
    let silent = (peaks.iter())
        .filter(|(_, dbfs)| *dbfs == SILENCE_DBFS)
        .count();
    assert_eq!(blips + silent, peaks.len(), "{peaks:?}");
    assert!((1..=3).contains(&blips), "{peaks:?}");
    assert!(silent >= 5 * blips, "{peaks:?}");
}

#[test]
fn the_meter_opens_the_mic_only_while_it_is_allowed() {
    // Not answered yet: only after the person's "Allow microphone" press. Refused: never. Nothing opens otherwise.
    for (permission, ask, opens) in [
        (Permission::Granted, false, true),
        (Permission::NotAsked, false, false),
        (Permission::Unknown, false, false),
        (Permission::NotAsked, true, true),
        (Permission::Unknown, true, true),
        (Permission::Denied, false, false),
        (Permission::Denied, true, false),
    ] {
        let backend = FakeBackend::new(Script {
            mic_permission: permission,
            ..sine()
        });
        let (code, events) = meter_on(&backend, ask, 300);
        let case = format!("{permission:?}, ask {ask}");
        assert_eq!(
            backend.opened().load(Ordering::SeqCst),
            u64::from(opens),
            "{case}"
        );
        if opens {
            assert_eq!(code, 0, "{case}");
            assert!(failed(&events).is_empty(), "{case}: {events:?}");
        } else {
            assert_eq!(code, 1, "{case}");
            assert_eq!(events.len(), 1, "{case}: {events:?}");
            assert_eq!(failed(&events), ["capture.permission.mic"], "{case}");
        }
    }
}

#[test]
fn the_meter_ends_when_the_mic_is_refused_while_it_runs() {
    // The person turns the mic off in the system's settings 300 ms in (a refused mic on macOS sends silence, never an
    // error): within about a second the meter ends with the microphone's step, and the mic closes.
    assert_eq!(PERMISSION_EVERY, Duration::from_secs(1));
    let backend = FakeBackend::new(sine());
    let permission = backend.mic_permission();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        *permission.lock().unwrap() = Permission::Denied;
    });
    let started = Instant::now();
    let (code, events) = meter_on(&backend, false, 10_000);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(code, 1);
    assert!(!peaks(&events).is_empty(), "{events:?}");
    assert_eq!(failed(&events), ["capture.permission.mic"]);
    assert!(matches!(events.last(), Some(Event::Failed { .. })));
}

#[test]
fn the_meter_ends_when_its_mic_goes() {
    // The mic is unplugged 300 ms in: its peaks came, then the app hears why, and nothing more.
    let script = Script {
        events: vec![(300, "mic".into(), PipeEvent::DeviceLost)],
        ..sine()
    };
    let (code, events) = meter(script, 5_000);
    assert_eq!(code, 1);
    assert!(!peaks(&events).is_empty(), "{events:?}");
    assert_eq!(failed(&events), ["capture.error.noDevice"]);
    assert!(matches!(events.last(), Some(Event::Failed { .. })));
}

/// Set only in the child process `meter_and_thumbs_write_no_file` starts, in an empty folder.
const CHILD: &str = "KLIPOLO_NO_FILE_CHILD";

/// Neither worker writes a file: in a child process (this test binary again, with only this test) whose working
/// folder, home, temporary folder and recordings root are one empty folder, the meter and the stills run, and the
/// folder is still empty afterwards. The child keeps the test run's `KLIPOLO_TEST`.
#[test]
fn meter_and_thumbs_write_no_file() {
    if std::env::var_os(CHILD).is_some() {
        let (code, events) = meter(sine(), 300);
        assert_eq!(code, 0);
        assert!(!peaks(&events).is_empty());
        let backend = FakeBackend::new(Script::default());
        let out = SharedOut::default();
        let io = PreviewIo::new(sent_after(&Command::Stop, 300), Box::new(out.clone()));
        let screen = [("display-1".to_string(), Target::Display { id: "1".into() })];
        assert_eq!(thumbs_with(&backend, &screen, io), 0);
        assert!(out.text().contains(r#""ev":"thumb""#));
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "meter_and_thumbs_write_no_file", "--nocapture"])
        .current_dir(dir.path())
        .env(CHILD, "1")
        .env("HOME", dir.path())
        .env("TMPDIR", dir.path())
        .env("KLIPOLO_RECORDINGS_ROOT", dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the child failed: {stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the child ran no test: {stdout}"
    );
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert!(left.is_empty(), "files written: {left:?}");
}

/// The fake backend, with every call to it counted.
struct Counted {
    inner: FakeBackend,
    calls: AtomicU64,
}

impl Counted {
    fn call(&self) -> &FakeBackend {
        self.calls.fetch_add(1, Ordering::SeqCst);
        &self.inner
    }
}

impl Backend for Counted {
    fn caps(&self) -> CaptureCaps {
        self.call().caps()
    }
    fn devices(&self) -> Devices {
        self.call().devices()
    }
    fn mics(&self) -> Vec<DeviceInfo> {
        self.call().mics()
    }
    fn permissions(&self) -> Permissions {
        self.call().permissions()
    }
    fn video(&self, spec: &VideoSpec) -> Result<VideoPipes, PipeError> {
        self.call().video(spec)
    }
    fn audio(&self, spec: &AudioSpec) -> Result<Box<dyn AudioPipe>, PipeError> {
        self.call().audio(spec)
    }
    fn cursor_source(&self, target: &Target) -> Result<Option<Box<dyn CursorSource>>, PipeError> {
        self.call().cursor_source(target)
    }
    fn preview(&self, camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        self.call().preview(camera)
    }
    fn live_view(&self, target: &Target) -> Result<Box<dyn PreviewPipe>, PipeError> {
        self.call().live_view(target)
    }
    fn still(&self, target: &Target, width: u32) -> Result<PreviewFrame, PipeError> {
        self.call().still(target, width)
    }
    fn probe(&self, budget: Duration) -> ProbeReport {
        self.call().probe(budget)
    }
    fn shell(&self) -> Box<dyn Shell> {
        self.call().shell()
    }
    fn display_awake(&self) -> bool {
        self.call().display_awake()
    }
    fn decodes(&self, info: &StreamInfo, keyframe: &[u8]) -> Option<bool> {
        self.call().decodes(info, keyframe)
    }
    fn request_screen_permission(&self) {
        self.call().request_screen_permission();
    }
}

#[test]
fn both_forms_stop_at_the_guard_in_a_test_run() {
    assert!(
        std::env::var_os("KLIPOLO_TEST").is_some(),
        "every test command sets KLIPOLO_TEST"
    );
    let backend = Counted {
        inner: FakeBackend::new(sine()),
        calls: AtomicU64::new(0),
    };
    let screen = [("display-1".to_string(), Target::Display { id: "1".into() })];
    let io = |out: &SharedOut| {
        let input = Box::new(BufReader::new(Cursor::new(vec![])));
        PreviewIo::new(input, Box::new(out.clone()))
    };
    let (thumbs_out, meter_out) = (SharedOut::default(), SharedOut::default());
    stops_at_the_guard("thumbs", || run_thumbs(&backend, &screen, io(&thumbs_out)));
    stops_at_the_guard("meter", || {
        run_meter(&backend, "mic-1", false, io(&meter_out))
    });
    assert_eq!(
        backend.calls.load(Ordering::SeqCst),
        0,
        "a call reached the backend"
    );
    assert_eq!(
        thumbs_out.text() + &meter_out.text(),
        "",
        "a worker said something"
    );
}

/// `run` panicked at the consent guard.
fn stops_at_the_guard(form: &str, run: impl FnOnce() -> i32) {
    let panic = catch_unwind(AssertUnwindSafe(run)).expect_err(form);
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("real capture in a test"),
        "{form}: {message}"
    );
}
