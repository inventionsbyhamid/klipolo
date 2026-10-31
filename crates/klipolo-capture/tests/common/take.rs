//! A synthetic take written the way the core writes one: the H.264 fixture's packets at 30 fps for the
//! screen and a synthetic mic, through the durable writer, with a flush tick every second of media (`write_take`).
//! The power cut `replay` recreates from the writer's log is in `replay.rs`.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use klipolo_capture::caf;
use klipolo_capture::drift::Anchor;
use klipolo_capture::fmp4::Fmp4Writer;
use klipolo_capture::io::{Io, LoggedIo, OsIo};
use klipolo_capture::journal::{self, Entry};
use klipolo_capture::media::{AudioFormat, EncodedPacket};
use klipolo_capture::run::core::{RunDone, RunKind};
use klipolo_capture::run::finish::finish_take;
use klipolo_capture::session::{self, SessionDir};
use klipolo_capture::synthetic::{AudioSource, FixtureVideo, Signal, SyntheticAudioSpec};
use klipolo_capture::writer::{DurableWriter, WriterSnapshot};
use klipolo_model::project::SourceRole;

use super::config::config_with;
// Some tests call it from here; most test binaries don't.
#[allow(unused_imports)]
pub use super::replay::replay;

pub const FPS: u32 = 30;
pub const RATE: u32 = 48_000;
pub const TIMING: &str = "timing.jsonl";

#[derive(Clone, Debug)]
pub struct Take {
    pub seconds: f64,
    /// Stop ran: the remux, the CAF's size, the manifest and `Finalized`. Without it every fragment is still written,
    /// the last one included (the muxer's flush), and the writer's last tick ran, but nothing more.
    pub finalize: bool,
    /// The last fragment cut in half after writing: a kill in the middle of its append.
    pub cut_last_fragment: bool,
    /// For the hour: real file IO without the log, and a tick every 10 s of media.
    pub fast: bool,
}

impl Default for Take {
    fn default() -> Self {
        Self {
            seconds: 2.0,
            finalize: false,
            cut_last_fragment: false,
            fast: false,
        }
    }
}

pub struct Written {
    pub dir: SessionDir,
    /// Every file step the writer took (empty for a `fast` take).
    pub io: Arc<LoggedIo>,
}

pub fn fixture() -> FixtureVideo {
    FixtureVideo::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gop-h264.mp4"))
}

pub fn mic_format() -> AudioFormat {
    AudioFormat {
        rate: RATE,
        channels: 1,
    }
}

/// `<root>/<id>.klipolorec`: a screen and a mic, one run each.
pub fn write_take(root: &Path, id: &str, take: Take) -> Written {
    let dir = SessionDir::create(root, id).unwrap();
    let mut cfg = config_with(&[("screen", SourceRole::Screen), ("mic", SourceRole::Mic)]);
    cfg.id = id.into();
    session::write_config(&dir, &cfg).unwrap();
    let logged = Arc::new(LoggedIo::new());
    let io: Arc<dyn Io> = if take.fast {
        Arc::new(OsIo)
    } else {
        logged.clone()
    };
    let w = DurableWriter::spawn(io.clone(), &dir).unwrap();
    let video = fixture();
    let format = mic_format();
    let mut mux = Fmp4Writer::at_rate(&video.info, FPS);
    let (screen, mic) = (
        SessionDir::run_key("screen", 1, true),
        SessionDir::run_key("mic", 1, false),
    );
    w.journal(&Entry::Started { host_ns: 0 }).unwrap();
    w.journal(&Entry::Gate { host_ns: 0 }).unwrap();
    w.open(&screen, mux.head()).unwrap();
    w.journal(&run_started("screen", &screen, Some(&video), None))
        .unwrap();
    w.open(&mic, caf::header(format)).unwrap();
    w.journal(&run_started("mic", &mic, None, Some(format)))
        .unwrap();
    w.open_lines(TIMING).unwrap();

    let spec = SyntheticAudioSpec {
        rate: RATE,
        channels: 1,
        block_ms: 10,
        drift_ppm: 0.0,
        signal: Signal::Tone {
            hz: 440.0,
            dbfs: -12.0,
        },
        blip_every_ms: None,
    };
    let mut sound = AudioSource::new(spec, 0, 0);
    // The running level the live recorder journals with each tick.
    let (mut mic_frames, mut mic_peak) = (0u64, 0f32);
    let total_us = (take.seconds * 1e6).round() as i64;
    let frames = (take.seconds * f64::from(FPS)).round() as u64;
    let tick_us = if take.fast { 10_000_000 } else { 1_000_000 };
    let pts = |i: u64| (i * 1_000_000 / u64::from(FPS)) as i64;
    let (mut next, mut t, mut pcm) = (0u64, 0i64, Vec::new());
    while t < total_us {
        let end = (t + 100_000).min(total_us);
        while next < frames && pts(next) < end {
            let (keyframe, data) = &video.frames[(next % video.frames.len() as u64) as usize];
            let packet = EncodedPacket {
                host_ns: pts(next) as u64 * 1_000,
                keyframe: *keyframe,
                data: data.clone(),
            };
            if let Some(f) = mux.push(pts(next), &packet) {
                let us = f.media_us();
                room(&w);
                w.append(&screen, f.bytes, us).unwrap();
            }
            next += 1;
        }
        let mut n = 0;
        while let Some(block) = sound.next_block(end as u64 * 1_000) {
            mic_peak = (block.samples.iter()).fold(mic_peak, |m, s| m.max(s.abs()));
            n += caf::encode_frames(format, &block, &mut pcm).unwrap();
        }
        mic_frames += n;
        room(&w);
        let us = (n * 1_000_000 / u64::from(RATE)) as i64;
        w.append(&mic, std::mem::take(&mut pcm), us).unwrap();
        if end % tick_us == 0 || end == total_us {
            let anchor = Anchor {
                session_us: end,
                frame: end as u64 * u64::from(RATE) / 1_000_000,
            };
            w.append(TIMING, journal::anchor_line("mic", 1, &anchor), 0)
                .unwrap();
            w.journal(&Entry::Levels {
                file: mic.clone(),
                peaks: vec![mic_peak],
                through: Some(caf::HEADER_LEN + mic_frames * caf::frame_bytes(format)),
            })
            .unwrap();
            w.tick().unwrap();
        }
        t = end;
    }
    let last = mux.flush().unwrap();
    let us = last.media_us();
    w.append(&screen, last.bytes, us).unwrap();
    if take.finalize {
        for source in ["screen", "mic"] {
            w.journal(&Entry::RunEnded {
                source: source.into(),
                run: 1,
                end_us: total_us,
            })
            .unwrap();
        }
        w.close(&screen).unwrap();
        w.close(&mic).unwrap();
    }
    let snap = w.finish().unwrap();
    assert!(snap.wrote_exactly(&screen, mux.bytes_written()));
    if take.cut_last_fragment {
        let moof_at = mux.index().fragments.last().unwrap().moof_at;
        let cut = moof_at + (mux.bytes_written() - moof_at) / 2;
        rw(&dir.path(&screen)).set_len(cut).unwrap();
    }
    if take.finalize {
        stop(&*io, &dir, &cfg, &mux, &snap);
    }
    Written { dir, io: logged }
}

fn run_started(
    source: &str,
    file: &str,
    video: Option<&FixtureVideo>,
    audio: Option<AudioFormat>,
) -> Entry {
    Entry::RunStarted {
        source: source.into(),
        run: 1,
        file: file.into(),
        offset_us: 0,
        video: video.map(|v| v.info.clone()),
        audio,
    }
}

/// Keeps the writer's queue short, as the real-time core does: the test makes media faster than the disk takes it.
fn room(w: &DurableWriter) {
    while w.snapshot().queued_ms > 2_000 {
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Stop after the writer's last tick, as the recorder does it (`run::finish`): the remux from the muxer's own index,
/// the CAF's size, the manifest and `Finalized`, every step through `io` (so the replay can cut inside Stop too).
fn stop(
    io: &dyn Io,
    dir: &SessionDir,
    cfg: &session::SessionConfig,
    mux: &Fmp4Writer,
    snap: &WriterSnapshot,
) {
    let runs = [
        RunDone {
            source: "screen".into(),
            run: 1,
            key: SessionDir::run_key("screen", 1, true),
            kind: RunKind::Video {
                info: mux.info().clone(),
                plan: mux.remux_plan(),
                bytes_written: mux.bytes_written(),
                duration_us: mux.index().duration_us(90_000),
            },
        },
        RunDone {
            source: "mic".into(),
            run: 1,
            key: SessionDir::run_key("mic", 1, false),
            kind: RunKind::Sound {
                format: mic_format(),
            },
        },
    ];
    finish_take(
        io,
        dir,
        cfg,
        &runs,
        &BTreeMap::new(),
        snap,
        &AtomicU64::new(0),
    )
    .unwrap();
}

pub fn rw(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap()
}
