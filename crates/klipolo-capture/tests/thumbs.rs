//! The Record dialog's stills of each screen and app in their worker, with the fake backend and a test clock: each
//! target once a second, in turn, at most 8 of them; a still that fails is tried again on its next turn, and one that
//! runs late moves the next turn back; the worker ends on the app's Stop or its stdin's end; without Screen Recording
//! it takes no still; and the Mac's stills read the permission first and never ask. Nothing captures.
#![cfg(feature = "synthetic")]

#[path = "common/preview_io.rs"]
#[allow(dead_code, reason = "the stills keep time by the test's own clock")]
mod preview_io;
// Only the source helpers, for the stills' consent scan.
#[path = "common/scan.rs"]
#[allow(
    dead_code,
    reason = "the stills' scan needs two of the shared source helpers"
)]
mod scan;

use std::io::{BufRead, BufReader, Cursor, Read};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use klipolo_capture::backend::{
    AudioSpec, Backend, CaptureCaps, Devices, Permission, Permissions, PreviewFrame, PreviewPipe,
    ProbeReport, Target, VideoPipes, VideoSpec,
};
use klipolo_capture::pipe::{AudioPipe, PipeError};
use klipolo_capture::protocol::{Command, Event};
use klipolo_capture::run::thumbs::{MAX_THUMBS, THUMB_EVERY, THUMB_WIDTH, thumbs_with};
use klipolo_capture::run::{Clock, PreviewIo};
use klipolo_capture::shell::Shell;
use klipolo_capture::synthetic::{FAKE_DISPLAY, FakeBackend, Script};
use preview_io::{Late, SharedOut, sent_after};
use scan::{code_of, read};

/// Time as the test says it: each wait ends at once, at the time it waits for, until `end`, when the worker's run
/// is over. Every wait is kept. Only the waits and a slow still (`SlowFirst`) move it.
struct TestClock {
    now: Arc<Mutex<Duration>>,
    end: Duration,
    waits: Arc<Mutex<Vec<Duration>>>,
}

impl Clock for TestClock {
    fn now(&self) -> Duration {
        *self.now.lock().unwrap()
    }

    fn wait_until(&mut self, at: Duration, stopped: &dyn Fn() -> bool) -> bool {
        self.waits.lock().unwrap().push(at);
        if at >= self.end || stopped() {
            return false;
        }
        let mut now = self.now.lock().unwrap();
        *now = (*now).max(at);
        true
    }
}

/// The app's stdin, open until the test drops the sender.
struct Open(Receiver<()>);

impl Read for Open {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        let _ = self.0.recv();
        Ok(0)
    }
}

fn open_input() -> (Sender<()>, Box<dyn BufRead + Send>) {
    let (open, rx) = channel();
    (open, Box::new(BufReader::new(Open(rx))))
}

/// `n` displays, keyed "display-1", "display-2", ….
fn displays(n: usize) -> Vec<(String, Target)> {
    (1..=n)
        .map(|i| {
            (
                format!("display-{i}"),
                Target::Display { id: i.to_string() },
            )
        })
        .collect()
}

/// What one run of the stills gave.
struct Run {
    code: i32,
    events: Vec<Event>,
    /// Each still the backend was asked for: its target and width.
    stills: Vec<(Target, u32)>,
    /// Each time the worker waited for.
    waits: Vec<Duration>,
}

/// The stills of `targets` over `secs` of the test clock, the app's stdin open meanwhile.
fn over(script: Script, targets: &[(String, Target)], secs: u64) -> Run {
    let backend = FakeBackend::new(script);
    over_on(&backend, &backend, Arc::default(), targets, secs)
}

/// `over` on `backend` (`fake` within it), with the test clock's time shared as `now`.
fn over_on(
    backend: &dyn Backend,
    fake: &FakeBackend,
    now: Arc<Mutex<Duration>>,
    targets: &[(String, Target)],
    secs: u64,
) -> Run {
    let stills = fake.stills();
    let out = SharedOut::default();
    let waits = Arc::default();
    let clock = TestClock {
        now,
        end: Duration::from_secs(secs),
        waits: Arc::clone(&waits),
    };
    let (open, input) = open_input();
    let io = PreviewIo {
        input,
        output: Box::new(out.clone()),
        clock: Box::new(clock),
    };
    let code = thumbs_with(backend, targets, io);
    drop(open);
    let stills = stills.lock().unwrap().clone();
    let waits = waits.lock().unwrap().clone();
    Run {
        code,
        events: out.events(),
        stills,
        waits,
    }
}

/// Each `Thumb` event's key, in order.
fn thumb_keys(events: &[Event]) -> Vec<&str> {
    (events.iter())
        .filter_map(|e| match e {
            Event::Thumb { key, .. } => Some(key.as_str()),
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

#[test]
fn thumbs_visit_each_target_once_a_second() {
    assert_eq!((THUMB_EVERY, THUMB_WIDTH), (Duration::from_secs(1), 320));
    let targets = displays(3);
    let run = over(Script::default(), &targets, 3);
    assert_eq!(run.code, 0);
    // 3 stills each, in turn, 320 pixels wide.
    let turns: Vec<Target> = (0..9).map(|k| targets[k % 3].1.clone()).collect();
    let asked: Vec<Target> = run.stills.iter().map(|(t, _)| t.clone()).collect();
    assert_eq!(asked, turns);
    assert!(run.stills.iter().all(|(_, w)| *w == THUMB_WIDTH));
    // Spread evenly over each second: a third of a second apart, so each target comes back a second later. The wait
    // at 3 s is where the test's time ends.
    let thirds: Vec<Duration> = (1..=9).map(|k| THUMB_EVERY * k / 3).collect();
    assert_eq!(run.waits, thirds);
    // Each still reaches the app under its target's key, as a JPEG in the target's shape.
    let keys: Vec<String> = (0..9).map(|k| format!("display-{}", k % 3 + 1)).collect();
    assert_eq!(thumb_keys(&run.events), keys);
    for event in &run.events {
        let Event::Thumb {
            jpeg_base64,
            width,
            height,
            ..
        } = event
        else {
            panic!("only stills: {event:?}");
        };
        let jpeg = base64::engine::general_purpose::STANDARD
            .decode(jpeg_base64)
            .unwrap();
        assert_eq!(jpeg, [0xff, 0xd8, 0xff, 0xd9]);
        assert_eq!(
            (*width, *height),
            (320, 320 * FAKE_DISPLAY.1 / FAKE_DISPLAY.0)
        );
    }
}

#[test]
fn thumbs_take_at_most_8_targets() {
    assert_eq!(MAX_THUMBS, 8);
    let targets = displays(10);
    let run = over(Script::default(), &targets, 2);
    assert_eq!(run.code, 0);
    // The first 8, each twice in 2 s; the 9th and 10th never.
    let first_8: Vec<Target> = (0..16).map(|k| targets[k % 8].1.clone()).collect();
    let asked: Vec<Target> = run.stills.iter().map(|(t, _)| t.clone()).collect();
    assert_eq!(asked, first_8);
    let keys = thumb_keys(&run.events);
    assert!(!keys.contains(&"display-9") && !keys.contains(&"display-10"));
    assert_eq!(keys.len(), 16);
}

#[test]
fn a_failed_still_is_retried_next_round() {
    // The first display's first still fails: it sends nothing, the second display's turn comes next, and the first is
    // asked again a second after it failed.
    let targets = displays(2);
    let script = Script {
        still_fails: vec![(targets[0].1.clone(), 1)],
        ..Script::default()
    };
    let run = over(script, &targets, 2);
    assert_eq!(run.code, 0);
    let turns: Vec<Target> = (0..4).map(|k| targets[k % 2].1.clone()).collect();
    let asked: Vec<Target> = run.stills.iter().map(|(t, _)| t.clone()).collect();
    assert_eq!(asked, turns);
    assert_eq!(
        thumb_keys(&run.events),
        ["display-2", "display-1", "display-2"]
    );
    assert!(
        failed(&run.events).is_empty(),
        "a failed still isn't the worker's end"
    );
}

/// The fake backend, its first still taking `.1` of the test clock's time; the clock's time at each still kept.
struct SlowFirst(
    FakeBackend,
    Duration,
    Arc<Mutex<Duration>>,
    Mutex<Vec<Duration>>,
);

impl Backend for SlowFirst {
    fn caps(&self) -> CaptureCaps {
        self.0.caps()
    }
    fn devices(&self) -> Devices {
        self.0.devices()
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
    fn preview(&self, camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        self.0.preview(camera)
    }
    fn still(&self, target: &Target, width: u32) -> Result<PreviewFrame, PipeError> {
        let mut at = self.3.lock().unwrap();
        let mut now = self.2.lock().unwrap();
        at.push(*now);
        if at.len() == 1 {
            *now += self.1;
        }
        self.0.still(target, width)
    }
    fn probe(&self, budget: Duration) -> ProbeReport {
        self.0.probe(budget)
    }
    fn shell(&self) -> Box<dyn Shell> {
        self.0.shell()
    }
}

#[test]
fn a_late_still_moves_the_next_turn_back() {
    // Two displays, a turn every half second; the first still takes 1.2 s. The next turn comes as it ends, and the
    // turns after it follow from there, half a second apart: they never bunch up to catch up.
    let now = Arc::default();
    let slow = SlowFirst(
        FakeBackend::new(Script::default()),
        Duration::from_millis(1_200),
        Arc::clone(&now),
        Mutex::default(),
    );
    let run = over_on(&slow, &slow.0, now, &displays(2), 3);
    assert_eq!(run.code, 0);
    let at: Vec<u64> = (slow.3.lock().unwrap().iter())
        .map(|t| t.as_millis() as u64)
        .collect();
    assert_eq!(at, [0, 1_200, 1_700, 2_200, 2_700]);
    assert_eq!(thumb_keys(&run.events).len(), 5);
}

/// One display's stills on real time, the app's lines from `input`: the exit code, how long it ran, and the stills
/// taken.
fn until_the_app_goes(input: Box<dyn BufRead + Send>) -> (i32, Duration, usize) {
    let backend = FakeBackend::new(Script::default());
    let stills = backend.stills();
    let io = PreviewIo::new(input, Box::new(SharedOut::default()));
    let started = Instant::now();
    let code = thumbs_with(&backend, &displays(1), io);
    let taken = stills.lock().unwrap().len();
    (code, started.elapsed(), taken)
}

#[test]
fn thumbs_stop_when_stdin_ends() {
    // The app goes 300 ms in (its stdin ends with no line), or says Stop then: the worker ends within a moment, long
    // before the display's next turn at 1 s, having taken only the first still.
    let ends = Box::new(BufReader::new(Late(Cursor::new(vec![]), 300)));
    let stop = sent_after(&Command::Stop, 300);
    for input in [ends as Box<dyn BufRead + Send>, stop] {
        let (code, took, taken) = until_the_app_goes(input);
        assert_eq!(code, 0);
        assert!(took < Duration::from_millis(900), "{took:?}");
        assert_eq!(taken, 1);
    }
}

#[test]
fn thumbs_refuse_without_screen_recording() {
    // Never a prompt: the permission is read first, and no still is taken.
    for screen_permission in [
        Permission::NotAsked,
        Permission::Denied,
        Permission::Unknown,
    ] {
        let script = Script {
            screen_permission,
            ..Script::default()
        };
        let run = over(script, &displays(2), 2);
        assert_eq!(run.code, 1);
        assert!(run.stills.is_empty());
        assert_eq!(failed(&run.events), ["capture.permission.screen"]);
    }
}

/// The Mac's stills read Screen Recording's preflight (which never prompts) before the content list (which can show
/// macOS's prompt) and the screenshot, and their worker reads the permission before it asks for a still. Neither
/// requests a permission, opens a camera or a mic, captures sound or writes a file.
#[test]
fn the_stills_read_the_permission_first_and_never_ask_or_open_a_camera_or_a_mic() {
    let mac = code_of(&read("platform/macos/thumbs.rs"));
    let guard = mac.find("assert_real_capture_allowed(");
    let preflight = mac.find("CGPreflightScreenCaptureAccess()");
    let content = mac.find("filter::content()").expect("the content list");
    let shot = mac.find("ScreenshotManager::").expect("the screenshot");
    assert!(
        guard.is_some_and(|g| preflight.is_some_and(|p| g < p)),
        "the guard comes first"
    );
    assert!(
        preflight.is_some_and(|p| p < content && content < shot),
        "the preflight comes before the content list, and the content list before the screenshot"
    );
    let worker = code_of(&read("run/thumbs.rs"));
    let permission = worker.find(".permissions()");
    let still = worker.find(".still(").expect("a still asked for");
    assert!(
        permission.is_some_and(|p| p < still),
        "the worker reads the permission before it asks for a still"
    );
    for (file, code) in [("Mac's stills", &mac), ("stills' worker", &worker)] {
        for banned in [
            "CGRequestScreenCaptureAccess",
            "request_screen_permission",
            "request_access",
            "requestAccess",
            "CaptureSession",
            "CaptureDevice",
            "AudioSpec",
            "cpal",
            "set_captures_audio",
            "SessionDir",
            "std::fs",
            "File::",
        ] {
            assert!(!code.contains(banned), "the {file} names {banned}");
        }
    }
}
