//! What the tests read from the harness: the journal, the in-memory files, and the files as a Stop now would leave
//! them (a clone of the core is stopped, so the muxers' last fragments are in), and the sync between a video source's
//! flashes and a sound source's blips after drift correction.

use std::collections::BTreeMap;

use klipolo_capture::drift::Anchor;
use klipolo_capture::fmp4::read;
use klipolo_capture::journal::{Entry, JournalRead};
use klipolo_capture::manifest::{self, Manifest};
use klipolo_capture::media::EncodedPacket;
use klipolo_capture::protocol::Command;
use klipolo_capture::run::core::Effect;
use klipolo_capture::session::FileKey;
use klipolo_capture::synthetic::frame_of;

use super::Harness;

/// A sample: (decode time in 90 kHz ticks, duration, keyframe, bytes).
type Sample = (u64, u32, bool, Vec<u8>);

impl Harness {
    /// When the gate opened, in seconds after the harness began.
    pub fn gate_s(&self) -> f64 {
        let at = self.journal().iter().find_map(|e| match e {
            Entry::Gate { host_ns } => Some(*host_ns),
            _ => None,
        });
        (at.expect("the gate opened") - self.base) as f64 / 1e9
    }

    pub fn file_keys(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }

    pub fn file_keys_with_prefix(&self, prefix: &str) -> Vec<String> {
        self.files
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect()
    }

    /// Whether `source`'s fake pipe was told to stop (one by one, or with every pipe).
    pub fn stopped(&self, source: &str) -> bool {
        self.fakes.iter().any(|f| f.id == source && f.stopped)
    }

    pub fn run_count(&self, source: &str) -> usize {
        self.journal()
            .iter()
            .filter(|e| matches!(e, Entry::RunStarted { source: s, .. } if s == source))
            .count()
    }

    pub fn run_offset_us(&self, source: &str, run: u32) -> i64 {
        self.journal()
            .iter()
            .find_map(|e| match e {
                Entry::RunStarted {
                    source: s,
                    run: r,
                    offset_us,
                    ..
                } if s == source && *r == run => Some(*offset_us),
                _ => None,
            })
            .expect("the run started")
    }

    /// The files as Stop would leave them now: a clone of the core is stopped and its appends applied to a copy.
    pub fn stopped_files(&self) -> BTreeMap<FileKey, Vec<u8>> {
        let mut core = self.core.clone();
        let mut files = self.files.clone();
        for e in core.command(Command::Stop, self.now) {
            if let Effect::Append { key, bytes, .. } = e {
                files.entry(key).or_default().extend_from_slice(&bytes);
            }
        }
        files
    }

    fn samples(files: &BTreeMap<FileKey, Vec<u8>>, key: &str) -> Vec<Sample> {
        read::samples(&files[key]).expect("the run file reads").1
    }

    /// The run's media span, as a Stop now would leave it.
    pub fn video_span_s(&self, key: &str) -> f64 {
        let s = Self::samples(&self.stopped_files(), key);
        let (first, last) = (s.first().unwrap(), s.last().unwrap());
        (last.0 + u64::from(last.1) - first.0) as f64 / 90_000.0
    }

    pub fn video_starts_on_a_keyframe(&self, key: &str) -> bool {
        Self::samples(&self.stopped_files(), key)
            .first()
            .is_some_and(|s| s.2)
    }

    /// The first frame written after each resume is a keyframe.
    pub fn video_starts_on_a_keyframe_after_resume(&self, key: &str) -> bool {
        let source = key.split('/').next().unwrap();
        let run: u32 = key[key.len() - 6..key.len() - 4].parse().unwrap();
        let offset = self.run_offset_us(source, run);
        let samples = Self::samples(&self.stopped_files(), key);
        let resumes: Vec<i64> = self
            .journal()
            .iter()
            .filter_map(|e| match e {
                Entry::Resume { at_us, .. } => Some(*at_us),
                _ => None,
            })
            .collect();
        !resumes.is_empty()
            && resumes.iter().all(|at| {
                let ticks = ((at - offset) as f64 * 0.09).round() as u64;
                samples.iter().find(|s| s.0 >= ticks).is_some_and(|s| s.2)
            })
    }

    /// After any missing frame (a drop, or a pause), the next frame written is a keyframe.
    pub fn no_delta_after_a_gap(&self, key: &str) -> bool {
        let frames: Vec<(u64, bool)> = Self::samples(&self.files, key)
            .into_iter()
            .map(|(_, _, keyframe, data)| {
                let (n, _) = frame_of(&EncodedPacket {
                    host_ns: 0,
                    keyframe,
                    data,
                })
                .expect("a synthetic frame");
                (n, keyframe)
            })
            .collect();
        let gaps = frames.windows(2).filter(|w| w[1].0 > w[0].0 + 1).count();
        assert!(gaps > 0, "the take had no gap to check");
        frames.windows(2).all(|w| w[1].0 == w[0].0 + 1 || w[1].1)
    }

    /// The manifest of the take as written so far (as `run::finish` derives it, with each file's own length).
    pub fn manifest(&self) -> Manifest {
        let mut anchors: BTreeMap<(String, u32), Vec<Anchor>> = BTreeMap::new();
        for line in self
            .files
            .get("timing.jsonl")
            .map_or(&[][..], Vec::as_slice)
            .split(|b| *b == b'\n')
        {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) {
                let key = (
                    v["source"].as_str().unwrap().to_owned(),
                    v["run"].as_u64().unwrap() as u32,
                );
                let anchor = Anchor {
                    session_us: v["sessionUs"].as_i64().unwrap(),
                    frame: v["frame"].as_u64().unwrap(),
                };
                anchors.entry(key).or_default().push(anchor);
            }
        }
        let durations = self
            .files
            .iter()
            .filter(|(k, _)| k.ends_with(".mp4") || k.ends_with(".caf"))
            .map(|(k, bytes)| {
                let us = if k.ends_with(".mp4") {
                    let s = Self::samples(&self.files, k);
                    (s.iter().map(|s| u64::from(s.1)).sum::<u64>() * 100 / 9) as i64
                } else {
                    let channels = if k.starts_with("mic/") { 1 } else { 2 };
                    ((bytes.len() as u64 - 68) / (3 * channels) * 1_000_000 / 48_000) as i64
                };
                (k.clone(), us)
            })
            .collect();
        let journal = JournalRead {
            entries: self.journal().to_vec(),
            ..JournalRead::default()
        };
        manifest::derive(&self.cfg, &journal, &anchors, &durations)
    }

    /// The worst distance, in ms, between a sound source's sync blips and the video's flash frames once both are
    /// placed on the recording clock (the sound by its fitted drift line).
    pub fn sync_error_ms(&self, video: &str, audio: &str) -> f64 {
        let m = self.manifest();
        let runs = |id: &str| {
            m.sources
                .iter()
                .find(|s| s.id == id)
                .expect("the source has runs")
                .runs
                .clone()
        };
        let mut flashes = vec![];
        for run in runs(video) {
            for (dts, _, keyframe, data) in Self::samples(&self.files, &run.file) {
                let (_, flash) = frame_of(&EncodedPacket {
                    host_ns: 0,
                    keyframe,
                    data,
                })
                .unwrap();
                if flash {
                    flashes.push(run.offset_us as f64 + dts as f64 / 0.09);
                }
            }
        }
        let mut worst: f64 = 0.0;
        let mut blips = 0;
        for run in runs(audio) {
            assert!(run.drift_ppm.abs() > 0.0 || !run.approximate, "{run:?}");
            let bytes = &self.files[&run.file][68..];
            let rate = 48_000.0 * (1.0 + run.drift_ppm * 1e-6);
            let mut quiet = usize::MAX;
            for (f, s) in bytes.chunks_exact(3).enumerate() {
                let v = (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0;
                if v.abs() <= 0.3 {
                    quiet = quiet.saturating_add(1);
                    continue;
                }
                if quiet > 480 {
                    let at = run.offset_us as f64 + f as f64 * 1e6 / rate;
                    let nearest = flashes
                        .iter()
                        .map(|t| (t - at).abs())
                        .fold(f64::MAX, f64::min);
                    if nearest < 500_000.0 {
                        worst = worst.max(nearest);
                        blips += 1;
                    }
                }
                quiet = 0;
            }
        }
        assert!(
            blips > 10 && !flashes.is_empty(),
            "{blips} blips and {} flashes paired",
            flashes.len()
        );
        worst / 1_000.0
    }
}
