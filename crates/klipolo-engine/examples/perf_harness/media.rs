//! `… -- media <job> <file>... [--seconds <s>] [--cache <folder>]`: what one of a file's media jobs costs, run alone
//! in this process so the peak memory is the job's. Jobs:
//! - `probe`: what an import reads (`probe_media`);
//! - `sound`: the sound cache and the waveform's peaks (`PcmStore::build`);
//! - `thumbs`: the filmstrip's keyframe scan, its chunks and the keyframe index (`ThumbStore`), after `sound` as the
//!   app runs it;
//! - `sound-decode`: the sound cache's decode alone (`audio::decode::decode`), every block dropped;
//! - `poster`: the file's poster (`make_poster`);
//! - `decode`: every file given decoded at once, frame after frame from the start for `--seconds` (10), as playback
//!   asks the preview's decoders (`spawn_decoder`): frames a second for each and together.
//!
//! Prints the wall and CPU time, the peak memory (resident on Linux, the footprint on a Mac) and what the job wrote
//! (files and MB) into `--cache` (a temporary folder by default, removed at the end).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use klipolo_engine::audio::pcm::{PcmKey, PcmStore};
use klipolo_engine::media::poster::make_poster;
use klipolo_engine::media::store::{ThumbStore, Want};
use klipolo_engine::render::probe::probe_media;
use klipolo_engine::vendor::cap::rendering::decoder::{DecoderTuning, spawn_decoder};
use klipolo_platform::durable::Os;

use super::measure::me;

/// The process's CPU time so far, user and system, in ms.
fn cpu_ms() -> (f64, f64) {
    // SAFETY: getrusage fills the struct it is given.
    let mut r: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut r) };
    let ms = |t: libc::timeval| t.tv_sec as f64 * 1e3 + t.tv_usec as f64 / 1e3;
    (ms(r.ru_utime), ms(r.ru_stime))
}

/// The process's peak memory in MB: its resident high-water mark on Linux, its peak footprint on a Mac.
pub fn peak_mb() -> f64 {
    if cfg!(target_os = "macos") {
        return me().peak_footprint as f64 / (1024.0 * 1024.0);
    }
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<f64>().ok())
        })
        .map_or(0.0, |kb| kb / 1024.0)
}

/// Files and bytes under `dir`.
fn written(dir: &Path) -> (usize, u64) {
    let mut n = 0;
    let mut bytes = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                stack.push(e.path());
            } else {
                n += 1;
                bytes += meta.len();
            }
        }
    }
    (n, bytes)
}

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while !done() {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

pub async fn run(args: &[String]) {
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let job = args.first().cloned().unwrap_or_default();
    let files: Vec<String> = args[1.min(args.len())..]
        .iter()
        .take_while(|a| !a.starts_with("--"))
        .map(|f| {
            Path::new(f)
                .canonicalize()
                .unwrap_or_else(|_| panic!("{f} is not there"))
                .display()
                .to_string()
        })
        .collect();
    let Some(file) = files.first().cloned() else {
        eprintln!(
            "usage: media probe|sound|thumbs|poster|decode <file>... [--seconds <s>] [--cache <folder>]"
        );
        std::process::exit(2);
    };
    let seconds: f64 = value("--seconds")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    let temp = tempfile::tempdir().expect("a temporary folder");
    let cache = value("--cache").map_or_else(|| temp.path().to_path_buf(), PathBuf::from);
    std::fs::create_dir_all(&cache).expect("the cache folder");
    let duration_us = probe_media(Path::new(&file)).map_or(0, |m| (m.duration * 1e6) as i64);
    let (cpu0, started) = (cpu_ms(), Instant::now());
    let note = match job.as_str() {
        "probe" => {
            let m = probe_media(Path::new(&file)).expect("probes");
            format!(
                "{:.0} s, {:?}, {} x {} at {:.2} fps",
                m.duration, m.kind, m.width, m.height, m.fps
            )
        }
        "sound" => {
            let pcm = PcmStore::new(cache.join("pcm"));
            let built = pcm.build(&PcmKey { path: file.clone() }).expect("builds");
            format!("{} frames", built.frames())
        }
        "thumbs" => {
            let pcm = PcmStore::new(cache.join("pcm"));
            let thumbs = ThumbStore::new(cache.join("thumbs"), Arc::new(Os));
            pcm.set_thumbs(thumbs.clone());
            pcm.build(&PcmKey { path: file.clone() }).expect("builds");
            let sound_ms = started.elapsed().as_secs_f64() * 1e3;
            let scan = Instant::now();
            thumbs.want(vec![Want {
                path: file.clone(),
                duration_us,
                at_us: 0,
            }]);
            // The scan starts on its own thread: wait for it to begin, then to end.
            wait_until(Duration::from_secs(5), || {
                thumbs.pending() || thumbs.scanning()
            });
            let done = wait_until(Duration::from_secs(3_600), || {
                !thumbs.pending() && !thumbs.scanning()
            });
            let strip = klipolo_engine::media::thumbs::strip(&cache.join("thumbs"), &file);
            format!(
                "sound first {sound_ms:.0} ms, then the scan {:.0} ms{}; {} chunks",
                scan.elapsed().as_secs_f64() * 1e3,
                if done { "" } else { " (timed out)" },
                strip.map_or(0, |s| s.chunks.len())
            )
        }
        "sound-decode" => {
            // The sound cache's decode alone, every block dropped: what the build adds is the rest.
            let mut frames = 0u64;
            klipolo_engine::audio::decode::decode(Path::new(&file), 0, None, &mut |_, s| {
                frames += s.len() as u64;
                Ok(true)
            })
            .expect("decodes");
            format!("{frames} samples")
        }
        "poster" => {
            let made = make_poster(&cache.join("thumbs"), &Os, &file).expect("makes a poster");
            if made.is_some() { "made" } else { "no picture" }.to_string()
        }
        "decode" => decode(&files, seconds).await,
        other => {
            eprintln!("unknown media job {other:?}");
            std::process::exit(2);
        }
    };
    let wall_ms = started.elapsed().as_secs_f64() * 1e3;
    let (n, bytes) = written(&cache);
    println!(
        "media {job} {}: {wall_ms:.0} ms wall, {:.0} ms CPU ({:.0} ms system), peak memory {:.0} MB, wrote {n} files ({:.1} MB); {note}",
        files
            .iter()
            .map(|f| Path::new(f)
                .file_name()
                .map_or(f.clone(), |n| n.to_string_lossy().to_string()))
            .collect::<Vec<_>>()
            .join(" + "),
        cpu_ms().0 + cpu_ms().1 - cpu0.0 - cpu0.1,
        cpu_ms().1 - cpu0.1,
        peak_mb(),
        bytes as f64 / 1e6,
    );
}

/// The decoders' thread names.
const NAMES: [&str; 4] = [
    "perf-decode-0",
    "perf-decode-1",
    "perf-decode-2",
    "perf-decode-3",
];

/// Every file decoded at once, frame after frame from the start for `seconds`: each file's frames a second.
async fn decode(files: &[String], seconds: f64) -> String {
    let started = Instant::now();
    let runs = files.iter().enumerate().map(|(i, file)| {
        let file = file.clone();
        async move {
            let fps = klipolo_engine::render::probe::probe_fps(Path::new(&file));
            let fps = if fps > 0.0 { fps } else { 30.0 };
            let handle = spawn_decoder(
                NAMES[i % NAMES.len()],
                PathBuf::from(&file),
                fps,
                0.0,
                false,
                DecoderTuning::default(),
            )
            .await
            .expect("a decoder");
            let frames = (seconds * fps) as usize;
            let mut got = 0usize;
            let from = Instant::now();
            for k in 0..frames {
                if handle.get_frame(k as f32 / fps as f32).await.is_some() {
                    got += 1;
                }
            }
            let secs = from.elapsed().as_secs_f64();
            (got, secs, format!("{:?}", handle.decoder_type()))
        }
    });
    let results = futures::future::join_all(runs).await;
    let wall = started.elapsed().as_secs_f64();
    let each: Vec<String> = results
        .iter()
        .map(|(got, secs, kind)| format!("{:.0} fps ({got} frames, {kind})", *got as f64 / secs))
        .collect();
    let total: usize = results.iter().map(|r| r.0).sum();
    format!(
        "{} together: {:.0} frames a second in all over {wall:.1} s",
        each.join(", "),
        total as f64 / wall
    )
}
