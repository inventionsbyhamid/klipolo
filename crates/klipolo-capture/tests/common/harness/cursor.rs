//! The harness for the cursor: a take whose cursor events the test sends itself, on the fake
//! clock, with the core polled ten times a second as the threads layer polls it; what the take wrote, read back; a
//! resumed worker over a killed fake take's files; and the scripted streams of the disk tests.
//!
//! `cursor` delivers one event at its moment without running the take up to it (nothing is polled in between), and
//! `run_to` runs the take on, polling each 100 ms: a test says when time passes.

use std::collections::BTreeMap;

use klipolo_capture::backend::VideoSpec;
use klipolo_capture::cursor::CursorEvent;
use klipolo_capture::io::OsIo;
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::preflight::cursor::CursorPlan;
use klipolo_capture::run::core::{Effect, PipeCmd, SessionCore};
use klipolo_capture::run::resume::resume_with;
use klipolo_capture::session::{CURSOR, FileKey, SessionConfig, SessionDir, SourceSpec};
use klipolo_cursor::{ClickSource, CursorTrack, Limits};

use super::{BASE_NS, Harness, Src, cfg, mics};

mod streams;
// The disk test's binary uses these.
#[allow(unused_imports)]
pub use streams::{continuous, cursor_bytes_per_hour, real_mix, real_motion, typical_mix};

/// The core's poll, as the threads layer's ticker sends it.
const POLL_NS: u64 = 100_000_000;

impl Harness {
    /// A take with a screen whose plan is `plan`, its cursor sent by the test (exact clicks; the area in points known
    /// before the gate, as a real source sends it first), started through the gate; `base` is the gate.
    pub fn recording_cursor(plan: CursorPlan) -> Self {
        Self::recording_cursor_with(plan, Some((1440.0, 900.0)))
    }

    /// `recording_cursor` with the area its source sends before the gate (None: none).
    pub fn recording_cursor_with(plan: CursorPlan, area: Option<(f32, f32)>) -> Self {
        let area = area.map(|(w_pts, h_pts)| CursorEvent::Area {
            host_ns: BASE_NS,
            w_pts,
            h_pts,
        });
        Self::recording_cursor_after(plan, area.as_slice())
    }

    /// `recording_cursor` whose source sends `early` before the gate (as Record starts it, during the countdown).
    pub fn recording_cursor_after(plan: CursorPlan, early: &[CursorEvent]) -> Self {
        let mut cfg = cfg(&[Src::Screen], 0);
        if let SourceSpec::Screen(v) = &mut cfg.sources[0].spec {
            v.cursor = plan;
        }
        let core =
            SessionCore::new(cfg.clone(), mics(), BASE_NS).with_cursor_clicks(ClickSource::Events);
        let mut h = Self::with_core(core, cfg, BASE_NS);
        h.fakes[0].pipe_cursor = false;
        for ev in early {
            let fx = h.core.cursor(ev.clone(), BASE_NS);
            h.apply(fx);
        }
        h.start_recording();
        h.base = gate_ns(h.journal()).expect("the gate opened");
        h.clear();
        h
    }

    /// Feeds one cursor event at `at_s` seconds after the gate (the fake clock moves there; nothing runs between).
    pub fn cursor(&mut self, at_s: f64, ev: CursorEvent) {
        self.now = self.now.max(self.at(at_s));
        let fx = self.core.cursor(ev, self.now);
        self.apply(fx);
    }

    /// Runs the take on to `at_s` seconds after the gate: its media and ticks in time order, and the core's poll at each
    /// 100 ms.
    pub fn run_to(&mut self, at_s: f64) {
        let end = self.at(at_s);
        while self.now < end {
            let next = self.base + ((self.now - self.base) / POLL_NS + 1) * POLL_NS;
            let to = next.min(end);
            while self.step(to) {}
            self.now = self.now.max(to);
            if to == next {
                let fx = self.core.poll(self.now);
                self.apply(fx);
            }
        }
    }

    /// The core's poll at `at_s` and nothing else: a poll that comes late (the core stalled), before what queued up.
    pub fn poll_at(&mut self, at_s: f64) {
        self.now = self.now.max(self.at(at_s));
        let fx = self.core.poll(self.now);
        self.apply(fx);
    }

    /// The screen pipe's answer to `CursorInFrames`, now.
    pub fn ack_cursor_in_frames(&mut self, ok: bool) {
        let fx = self.core.cursor_in_frames(ok, self.now, self.now);
        self.apply(fx);
    }

    /// `cursor.jsonl` as written so far, read back.
    pub fn cursor_track(&self) -> CursorTrack {
        let bytes = self.files.get(CURSOR).map_or(&[][..], Vec::as_slice);
        CursorTrack::read(bytes, Limits::for_file_len(bytes.len() as u64))
    }

    /// The `PipeCmd`s sent to `source` since the gate, in order.
    pub fn pipe_cmds(&self, source: &str) -> Vec<PipeCmd> {
        (self.effects.iter())
            .filter_map(|e| match e {
                Effect::Pipe { source: s, cmd } if s == source => Some(*cmd),
                _ => None,
            })
            .collect()
    }

    /// The codes of the notes the take gave (its journal's).
    pub fn note_codes(&self) -> Vec<String> {
        (self.journal.iter())
            .filter_map(|e| match e {
                Entry::Health { code, .. } => Some(code.clone()),
                _ => None,
            })
            .collect()
    }

    /// The screen source's spec as the core builds its pipe now (a resume or a heal may have changed its plan).
    pub fn screen_spec(&self) -> VideoSpec {
        let cfg = self.core.pipe_config();
        cfg.sources
            .iter()
            .find_map(|s| match &s.spec {
                SourceSpec::Screen(v) => Some(v.clone()),
                _ => None,
            })
            .expect("a screen")
    }

    /// A worker resumed `at_s` seconds after the gate over a killed fake take's `files` (its journal and config among
    /// them), through `run::resume` on a real folder, as the worker resumes.
    pub fn resume_from(files: &BTreeMap<FileKey, Vec<u8>>, at_s: f64) -> Self {
        let root = tempfile::tempdir().unwrap();
        let dir = SessionDir::create(root.path(), "take").unwrap();
        for (key, bytes) in files {
            let path = dir.path(key);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let cfg: SessionConfig = serde_json::from_slice(&files["session.json"]).unwrap();
        let journal = journal::read(&dir.journal()).unwrap().entries;
        let gate = gate_ns(&journal).expect("the killed take passed its gate");
        let now = gate + (at_s * 1e9) as u64;
        let (lock, from) = resume_with(&dir, &OsIo, now).unwrap();
        let core = SessionCore::resumed(cfg.clone(), mics(), from, now)
            .with_cursor_clicks(ClickSource::Events);
        drop(lock);
        let mut kept = BTreeMap::new();
        for key in files
            .keys()
            .filter(|k| !k.ends_with(".json") && !k.ends_with(".lock"))
        {
            if let Ok(bytes) = std::fs::read(dir.path(key)) {
                kept.insert(key.clone(), bytes);
            }
        }
        kept.retain(|k, _| k != "journal.jsonl");
        let mut h = Self::with_core(core, cfg, now);
        h.base = gate;
        h.fakes[0].pipe_cursor = false;
        // What the resumed core wrote as it opened lands after the take's earlier files' bytes.
        for (key, bytes) in std::mem::take(&mut h.files) {
            kept.entry(key).or_default().extend(bytes);
        }
        h.files = kept;
        h
    }

    /// The take as a crashed worker leaves it: every file written so far, the journal and the config.
    pub fn killed_files(&self) -> BTreeMap<FileKey, Vec<u8>> {
        let mut files = self.files.clone();
        let journal: Vec<u8> = self.journal.iter().flat_map(journal::line).collect();
        files.insert("journal.jsonl".into(), journal);
        files.insert(
            "session.json".into(),
            serde_json::to_vec(&self.cfg).unwrap(),
        );
        files
    }

    fn at(&self, at_s: f64) -> u64 {
        self.base + (at_s.max(0.0) * 1e9) as u64
    }
}

fn gate_ns(journal: &[Entry]) -> Option<u64> {
    journal.iter().find_map(|e| match e {
        Entry::Gate { host_ns } => Some(*host_ns),
        _ => None,
    })
}

fn mv(h: &Harness, at_s: f64, x: f32) -> CursorEvent {
    CursorEvent::Move {
        host_ns: h.at(at_s),
        x,
        y: 0.5,
    }
}

/// A take that burned in at `at_s` (its source silent from 1.5 s before, the stream confirming at once), then was
/// killed half a second later.
pub fn burned_take(at_s: f64) -> BTreeMap<FileKey, Vec<u8>> {
    let mut h = decided(at_s);
    h.ack_cursor_in_frames(true);
    h.run_to(at_s + 0.5);
    h.killed_files()
}

/// A take whose watchdog decided to burn in at `at_s`, killed half a second later before the stream answered.
pub fn decided_take(at_s: f64) -> BTreeMap<FileKey, Vec<u8>> {
    let mut h = decided(at_s);
    h.run_to(at_s + 0.5);
    h.killed_files()
}

fn decided(at_s: f64) -> Harness {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    let last = at_s - 1.5;
    let mut t = 0.0;
    while t <= last + 1e-9 {
        h.cursor(t, mv(&h, t, 0.2 + 0.05 * t as f32));
        t += 0.1;
    }
    h.run_to(at_s);
    assert_eq!(h.pipe_cmds("screen"), [PipeCmd::CursorInFrames]);
    h
}

/// A take with `n` shapes written, its cursor alive, then killed at 2.5 s.
pub fn take_with_shapes(n: u16) -> BTreeMap<FileKey, Vec<u8>> {
    let mut h = Harness::recording_cursor(CursorPlan::Separate);
    for i in 0..n {
        let t = 0.2 + 0.2 * f64::from(i);
        h.cursor(t, mv(&h, t, 0.3));
        let image = crate::common::shape(8 + i, 8);
        let host_ns = h.at(t);
        h.cursor(
            t,
            CursorEvent::NewShape {
                host_ns,
                key: 1_000 + u64::from(i),
                image,
            },
        );
    }
    // Alive (a beat each 500 ms) until killed at 2.5 s, past the screen's first fragments.
    for t in [0.6, 1.1, 1.6, 2.1] {
        let host_ns = h.at(t);
        h.cursor(
            t,
            CursorEvent::Beat {
                host_ns,
                x: 0.3,
                y: 0.5,
            },
        );
    }
    h.run_to(2.5);
    h.killed_files()
}
