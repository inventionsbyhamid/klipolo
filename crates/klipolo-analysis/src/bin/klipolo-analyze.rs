//! A development tool for smoke tests against the real API, never part of the app: `klipolo-analyze transcript
//! <video>` writes the clip's transcript to `transcript.json` and prints what it cost.
//!
//! ```text
//! klipolo-analyze transcript <video> [--out <folder>] [--keyword <word>]...
//! ```
//!
//! It calls Meta for real, through the Meta adapter. `MODEL_API_KEY` (a Meta key) or `KLIPOLO_RELAY_TOKEN` (a relay
//! device token) comes from the environment and is never printed. With both, Meta is called with the key and the
//! relay token times the words; with the key alone, word times are approximate. `KLIPOLO_SPEND_LEDGER` (a CSV file),
//! when set, gets one row per paid call; `KLIPOLO_META_BASE` and `KLIPOLO_RELAY_BASE` point the calls elsewhere.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use klipolo_analysis::audio::extract_wav_16k;
use klipolo_analysis::cache::content_key;
use klipolo_analysis::providers::Transcriber;
use klipolo_analysis::providers::meta::Meta;
use klipolo_analysis::transcript::{TranscriptOut, transcribe_asset};
use klipolo_engine::render::probe::probe_media;
use klipolo_meta::{MetaClient, SPEND_LEDGER_ENV, SecretString};
use klipolo_model::speech::WordKind;
use klipolo_model::time::secs_to_us;

const KEY_ENV: &str = "MODEL_API_KEY";
const TOKEN_ENV: &str = "KLIPOLO_RELAY_TOKEN";
const USAGE: &str =
    "usage: klipolo-analyze transcript <video> [--out <folder>] [--keyword <word>]...";

struct Args {
    video: PathBuf,
    out: PathBuf,
    keywords: Vec<String>,
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("klipolo-analyze: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    if args.next().as_deref() != Some("transcript") {
        return Err(USAGE.into());
    }
    let (mut video, mut out, mut keywords) = (None, PathBuf::from("."), Vec::new());
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = args.next().map(PathBuf::from).ok_or(USAGE)?,
            "--keyword" => keywords.push(args.next().ok_or(USAGE)?),
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}\n{USAGE}")),
            _ if video.is_none() => video = Some(PathBuf::from(arg)),
            _ => return Err(USAGE.into()),
        }
    }
    Ok(Args {
        video: video.ok_or(USAGE)?,
        out,
        keywords,
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

fn run(args: &Args) -> Result<(), String> {
    let (client, credentials) = client()?;
    let meta = Meta::new(client);
    let key = content_key(&args.video)
        .map_err(|e| format!("Could not read {}: {e}", args.video.display()))?;
    std::fs::create_dir_all(&args.out)
        .map_err(|e| format!("Could not make {}: {e}", args.out.display()))?;
    let temp = TempDir::new()?;
    let wav = temp.0.join("sound.wav");
    let sound = extract_wav_16k(&args.video, &wav).map_err(|e| e.to_string())?;
    let mut out = if sound.has_audio {
        // One thread is plenty: the requests wait on the network together.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("Could not start: {e}"))?;
        runtime
            .block_on(transcribe_asset(&meta, &meta, &wav, &args.keywords))
            .map_err(|e| format!("{} ({e})", e.user_message()))?
    } else {
        // No sound stream: nothing to hear, over the whole clip.
        let clip = probe_media(&args.video).map_err(|e| format!("Could not read the clip: {e}"))?;
        TranscriptOut::no_speech(meta.model(), secs_to_us(clip.duration), 0.0)
    };
    out.transcript.key = key;
    let path = args.out.join("transcript.json");
    let json = serde_json::to_vec_pretty(&out).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    let sound_us = sound.has_audio.then_some(sound.duration_us);
    print_summary(&args.video, &out, sound_us, credentials, &path);
    Ok(())
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
