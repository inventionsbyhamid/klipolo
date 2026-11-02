//! What analysis's local steps cost on one file, one step per run so the peak memory is the step's (no Meta call):
//!
//! ```text
//! cargo run -p klipolo-analysis --release --example local_steps -- <video> key|wav|chunks|cuts [--out <folder>]
//! ```
//!
//! - `key`: the file's content key (`cache::content_key`);
//! - `wav`: the 16 kHz WAV for speech (`audio::extract_wav_16k`), into `--out` (a temporary folder by default);
//! - `chunks`: the WAV made as `wav` does, then read and cut into transcription requests (`transcript::Sound`), as
//!   transcribing does before its first request: the requests and what the read sound holds;
//! - `cuts`: the file's hard cuts from every frame (`cuts::detect`), the decoding the small copy's cut finder does.
//!
//! Prints the wall time, and on Linux the CPU time and the peak resident memory (on a Mac, run it under
//! `/usr/bin/time -l` for those).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use klipolo_analysis::audio::{extract_wav_16k, read_pcm_file};
use klipolo_analysis::cache::content_key;
use klipolo_analysis::cuts;
use klipolo_analysis::transcript::Sound;

/// What Voice Transcribe takes at most per request (s).
const MAX_AUDIO_S: u32 = 600;

/// The process's CPU ms and peak resident MB (Linux; zeros elsewhere).
fn usage() -> (f64, f64) {
    let cpu = std::fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|s| {
            // After the command's name: utime and stime are fields 14 and 15, in clock ticks (100 a second).
            let rest = s.rsplit_once(')')?.1;
            let f: Vec<&str> = rest.split_whitespace().collect();
            Some((f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?) * 10.0)
        })
        .unwrap_or(0.0);
    let peak = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<f64>().ok())
        })
        .map_or(0.0, |kb| kb / 1024.0);
    (cpu, peak)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(video), Some(step)) = (args.first(), args.get(1)) else {
        eprintln!("usage: local_steps <video> key|wav|chunks|cuts [--out <folder>]");
        std::process::exit(2);
    };
    let video = Path::new(video);
    let temp = tempfile::tempdir().expect("a temporary folder");
    let out: PathBuf = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map_or_else(|| temp.path().to_path_buf(), PathBuf::from);
    std::fs::create_dir_all(&out).expect("the folder");
    let wav = out.join("speech-16k.wav");
    let started = Instant::now();
    let note = match step.as_str() {
        "key" => content_key(video).expect("a key"),
        "wav" => {
            let info = extract_wav_16k(video, &wav).expect("a WAV");
            format!(
                "{:.0} s of sound, {:.1} MB of WAV",
                info.duration_us as f64 / 1e6,
                std::fs::metadata(&wav).map_or(0, |m| m.len()) as f64 / 1e6
            )
        }
        "chunks" => {
            extract_wav_16k(video, &wav).expect("a WAV");
            let made = started.elapsed().as_secs_f64() * 1e3;
            let read = Instant::now();
            let pcm = read_pcm_file(&wav).expect("reads the WAV");
            let held = pcm.samples.len() * 2;
            let sound = Sound::from_pcm(pcm, MAX_AUDIO_S);
            format!(
                "WAV {made:.0} ms, then read and cut {:.0} ms: {} requests over {:.0} s, {:.1} MB of sound held",
                read.elapsed().as_secs_f64() * 1e3,
                sound.chunks.len(),
                sound.duration_us as f64 / 1e6,
                held as f64 / 1e6
            )
        }
        "cuts" => {
            let found = cuts::detect(video, &AtomicBool::new(false)).expect("decodes");
            format!("{} cuts", found.len())
        }
        other => {
            eprintln!("unknown step {other:?}");
            std::process::exit(2);
        }
    };
    let (cpu, peak) = usage();
    println!(
        "analysis {step} {}: {:.0} ms wall, {cpu:.0} ms CPU, peak resident {peak:.0} MB; {note}",
        video
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().to_string()),
        started.elapsed().as_secs_f64() * 1e3,
    );
}
