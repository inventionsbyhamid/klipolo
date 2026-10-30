//! A development tool for smoke tests against the real API, never part of the app: `klipolo-analyze transcript
//! <video>` writes the clip's transcript to `transcript.json`, and `klipolo-analyze scenes <video>` its scene log to
//! `scenes.json`; each prints what it cost.
//!
//! ```text
//! klipolo-analyze transcript <video> [--out <folder>] [--keyword <word>]...
//! klipolo-analyze scenes <video> [--out <folder>] [--transcript <transcript.json>] [--proxy <mp4>]
//! ```
//!
//! It calls Meta for real, through the Meta adapter. `MODEL_API_KEY` (a Meta key) or `KLIPOLO_RELAY_TOKEN` (a relay
//! device token) comes from the environment and is never printed. With both, Meta is called with the key and the
//! relay token times the words; with the key alone, word times are approximate. `KLIPOLO_SPEND_LEDGER` (a CSV file),
//! when set, gets one row per paid call; `KLIPOLO_META_BASE` and `KLIPOLO_RELAY_BASE` point the calls elsewhere.
//!
//! `scenes` makes the clip's small copy in a temporary folder (a Mac only: Linux has no H.264 encoder), or uses a ready
//! one (`--proxy`, at most ten minutes), and snaps the log to the words of a `transcript.json` when given one.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use klipolo_analysis::audio::extract_wav_16k;
use klipolo_analysis::cache::content_key;
use klipolo_analysis::providers::Transcriber;
use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::proxy::{self, ProxyInfo};
use klipolo_analysis::scenes::{self, SceneLog};
use klipolo_analysis::transcript::{TranscriptOut, transcribe_asset};
use klipolo_engine::render::probe::probe_media;
use klipolo_meta::{MetaClient, SPEND_LEDGER_ENV, SecretString};
use klipolo_model::speech::{Word, WordKind};
use klipolo_model::time::secs_to_us;

const KEY_ENV: &str = "MODEL_API_KEY";
const TOKEN_ENV: &str = "KLIPOLO_RELAY_TOKEN";
const USAGE: &str = "usage: klipolo-analyze transcript <video> [--out <folder>] [--keyword <word>]...
       klipolo-analyze scenes <video> [--out <folder>] [--transcript <transcript.json>] [--proxy <mp4>]";

enum Args {
    Transcript {
        video: PathBuf,
        out: PathBuf,
        keywords: Vec<String>,
    },
    Scenes {
        video: PathBuf,
        out: PathBuf,
        transcript: Option<PathBuf>,
        proxy: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let ran = match &args {
        Args::Transcript {
            video,
            out,
            keywords,
        } => run(video, out, keywords),
        Args::Scenes {
            video,
            out,
            transcript,
            proxy,
        } => run_scenes(video, out, transcript.as_deref(), proxy.as_deref()),
    };
    match ran {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("klipolo-analyze: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let command = args.next().ok_or(USAGE)?;
    if command != "transcript" && command != "scenes" {
        return Err(USAGE.into());
    }
    let (mut video, mut out, mut keywords) = (None, PathBuf::from("."), Vec::new());
    let (mut transcript, mut proxy) = (None, None);
    let scenes = command == "scenes";
    while let Some(arg) = args.next() {
        let path = |next: Option<String>| next.map(PathBuf::from).ok_or(USAGE);
        match arg.as_str() {
            "--out" => out = path(args.next())?,
            "--keyword" if !scenes => keywords.push(args.next().ok_or(USAGE)?),
            "--transcript" if scenes => transcript = Some(path(args.next())?),
            "--proxy" if scenes => proxy = Some(path(args.next())?),
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}\n{USAGE}")),
            _ if video.is_none() => video = Some(PathBuf::from(arg)),
            _ => return Err(USAGE.into()),
        }
    }
    let video = video.ok_or(USAGE)?;
    Ok(if scenes {
        Args::Scenes {
            video,
            out,
            transcript,
            proxy,
        }
    } else {
        Args::Transcript {
            video,
            out,
            keywords,
        }
    })
}

/// The client the environment asks for, and which credentials it used (never their values).
fn client() -> Result<(MetaClient, &'static str), String> {
    let var = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    match (var(KEY_ENV), var(TOKEN_ENV)) {
        (Some(key), Some(token)) => Ok((
            MetaClient::direct(SecretString::from(key)).with_align_token(SecretString::from(token)),
            "Meta key; relay token for word timing",
        )),
        (Some(key), None) => Ok((
            MetaClient::direct(SecretString::from(key)),
            "Meta key; no relay token, so word times are approximate",
        )),
        (None, Some(token)) => Ok((MetaClient::relay(SecretString::from(token)), "relay token")),
        (None, None) => Err(format!("Set {KEY_ENV} or {TOKEN_ENV}.")),
    }
}

/// A folder of our own under the system's temporary folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("klipolo-analyze-{}", std::process::id()));
        std::fs::create_dir_all(&path)
            .map_err(|e| format!("Could not make {}: {e}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(video: &Path, out_dir: &Path, keywords: &[String]) -> Result<(), String> {
    let (client, credentials) = client()?;
    let meta = Meta::new(client);
    let key = content_key(video).map_err(|e| format!("Could not read {}: {e}", video.display()))?;
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("Could not make {}: {e}", out_dir.display()))?;
    let temp = TempDir::new()?;
    let wav = temp.0.join("sound.wav");
    let sound = extract_wav_16k(video, &wav).map_err(|e| e.to_string())?;
    let mut out = if sound.has_audio {
        // One thread is plenty: the requests wait on the network together.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("Could not start: {e}"))?;
        runtime
            .block_on(transcribe_asset(&meta, &meta, &wav, keywords))
            .map_err(|e| format!("{} ({e})", e.user_message()))?
    } else {
        // No sound stream: nothing to hear, over the whole clip.
        let clip = probe_media(video).map_err(|e| format!("Could not read the clip: {e}"))?;
        TranscriptOut::no_speech(Transcriber::model(&meta), secs_to_us(clip.duration), 0.0)
    };
    out.transcript.key = key;
    let path = out_dir.join("transcript.json");
    let json = serde_json::to_vec_pretty(&out).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    let sound_us = sound.has_audio.then_some(sound.duration_us);
    print_summary(video, &out, sound_us, credentials, &path);
    Ok(())
}

fn run_scenes(
    video: &Path,
    out_dir: &Path,
    transcript: Option<&Path>,
    ready: Option<&Path>,
) -> Result<(), String> {
    let (client, credentials) = client()?;
    let meta = Meta::new(client);
    let clip = probe_media(video).map_err(|e| format!("Could not read the clip: {e}"))?;
    let words = match transcript {
        Some(path) => words_of(path)?,
        None => Vec::new(),
    };
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("Could not make {}: {e}", out_dir.display()))?;
    let temp = TempDir::new()?;
    let copy = match ready {
        Some(path) => {
            let mut copy =
                proxy::existing(path).map_err(|e| format!("{} ({e})", e.user_message()))?;
            // The words are timed in the clip's sound, wherever the copy came from.
            copy.audio_lead_us = clip.audio_lead_us;
            copy
        }
        None => proxy::make(video, &temp.0, proxy::FIT.1, &AtomicBool::new(false))
            .map_err(|e| format!("{} ({e})", e.user_message()))?,
    };
    let estimate = scenes::estimate_usd(&meta, copy.duration_us);
    print_copy(video, credentials, &copy, words.len(), estimate);
    // One thread is plenty: the calls wait on the network, the cut search runs on the blocking pool.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Could not start: {e}"))?;
    let log = runtime
        .block_on(scenes::log(&meta, &copy, &words))
        .map_err(|e| format!("{} ({e})", e.user_message()))?;
    let path = out_dir.join("scenes.json");
    let json = serde_json::to_vec_pretty(&log).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    print_log(&log, &path);
    Ok(())
}

/// The words of a `transcript.json` (only `transcript.words` is read).
fn words_of(path: &Path) -> Result<Vec<Word>, String> {
    let text =
        std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let mut saved: serde_json::Value = serde_json::from_slice(&text)
        .map_err(|e| format!("{} is not JSON: {e}", path.display()))?;
    serde_json::from_value(saved["transcript"]["words"].take())
        .map_err(|e| format!("{} has no transcript words: {e}", path.display()))
}

fn print_copy(video: &Path, credentials: &str, copy: &ProxyInfo, words: usize, estimate: f64) {
    println!(
        "{}",
        video.file_name().unwrap_or_default().to_string_lossy()
    );
    println!("  credentials: {credentials}");
    println!(
        "  clip: {:.1} s; sound starts {:.3} s after the picture",
        copy.duration_us as f64 / 1e6,
        copy.audio_lead_us as f64 / 1e6
    );
    let bytes: u64 = copy.chunks.iter().map(|c| c.bytes).sum();
    println!(
        "  copy: {} chunk(s), {:.1} MB, {}x{} at {:.2} fps, sound: {}",
        copy.chunks.len(),
        bytes as f64 / 1e6,
        copy.width,
        copy.height,
        copy.fps,
        if copy.has_audio { "yes" } else { "no" }
    );
    for chunk in &copy.chunks {
        println!(
            "    {:.1} to {:.1} s: {:.1} MB",
            chunk.range.start as f64 / 1e6,
            chunk.range.end as f64 / 1e6,
            chunk.bytes as f64 / 1e6
        );
    }
    println!("  words to snap to: {words}");
    println!(
        "  watched at {} fps; estimate: ${estimate:.6}",
        scenes::fps_for(copy.duration_us)
    );
}

fn print_log(log: &SceneLog, path: &Path) {
    let secs = |us: i64| us as f64 / 1e6;
    let cuts: Vec<String> = log
        .cuts
        .iter()
        .map(|&c| format!("{:.3}", secs(c)))
        .collect();
    println!("  cuts: {} [{}]", log.cuts.len(), cuts.join(", "));
    println!("  items: {}", log.items.len());
    for item in &log.items {
        let who = if item.who.is_empty() {
            String::new()
        } else {
            format!(" ({})", item.who)
        };
        let shown = if item.on_screen_text.is_empty() {
            String::new()
        } else {
            format!(" [on screen: {}]", item.on_screen_text)
        };
        println!(
            "    {:7.3} to {:7.3} s {:?}{who}: {}{shown}",
            secs(item.start_us),
            secs(item.end_us),
            item.kind,
            item.text
        );
    }
    println!("  model: {}", log.model);
    println!("  cost: ${:.6}", log.cost_usd);
    match std::env::var_os(SPEND_LEDGER_ENV).filter(|v| !v.is_empty()) {
        Some(ledger) => println!("  ledger: {}", Path::new(&ledger).display()),
        None => println!("  ledger: none ({SPEND_LEDGER_ENV} is not set)"),
    }
    println!("  wrote {}", path.display());
}

fn print_summary(
    video: &Path,
    out: &TranscriptOut,
    sound_us: Option<i64>,
    credentials: &str,
    path: &Path,
) {
    let words = &out.transcript.words;
    let count = |kind: WordKind| words.iter().filter(|w| w.kind == kind).count();
    let fillers: BTreeSet<String> = words
        .iter()
        .filter(|w| w.kind == WordKind::Filler)
        .map(|w| {
            w.text
                .chars()
                .filter(|c| c.is_alphabetic())
                .flat_map(char::to_lowercase)
                .collect()
        })
        .collect();
    let requests = out.turns.iter().map(|t| t.chunk + 1).max().unwrap_or(0);
    println!(
        "{}",
        video.file_name().unwrap_or_default().to_string_lossy()
    );
    println!("  credentials: {credentials}");
    match sound_us {
        Some(us) => println!("  sound: {:.1} s", us as f64 / 1e6),
        None => println!("  sound: none (the clip has no sound stream)"),
    }
    if let Some(reason) = &out.empty_reason {
        println!("  {reason}");
    } else {
        println!(
            "  turns: {} (heard in {requests} request(s)), {} timed by the aligner",
            out.turns.len(),
            out.turns.iter().filter(|t| t.aligned).count()
        );
        println!(
            "  words: {}, fillers: {} {:?}, pauses: {}",
            count(WordKind::Word),
            count(WordKind::Filler),
            fillers,
            count(WordKind::Pause)
        );
        match &out.approximate_reason {
            Some(reason) => println!("  word timing: approximate ({reason})"),
            None => println!("  word timing: exact"),
        }
    }
    println!("  cost: ${:.6}", out.cost_usd);
    match std::env::var_os(SPEND_LEDGER_ENV).filter(|v| !v.is_empty()) {
        Some(ledger) => println!("  ledger: {}", Path::new(&ledger).display()),
        None => println!("  ledger: none ({SPEND_LEDGER_ENV} is not set)"),
    }
    println!("  wrote {}", path.display());
}
