//! A development tool for the smoke test against the real API, never part of the app: runs the app's own analysis
//! (`klipolo_app::analysis::AnalysisState`, the pipeline the Analyze button starts) on clips, and prints the estimate,
//! every event the page would hear, what was found and what it cost.
//!
//! ```text
//! cargo run -p klipolo-app --example analyze -- <clip>... [--cache <folder>]
//! ```
//!
//! The person's Meta key comes from `MODEL_API_KEY`, or an access code from `KLIPOLO_ACCESS_CODE`; neither is printed.
//! They are held in memory only (never the Keychain). The device registers with the relay on first use, as the app does
//! (word timing), under an id kept in `<folder>/device.json`. `KLIPOLO_SPEND_LEDGER` (a CSV file), when set, gets one
//! row per paid call; `KLIPOLO_META_BASE` and `KLIPOLO_RELAY_BASE` point the calls elsewhere.
//!
//! `--cache` keeps the analysis cache, the sound cache and the device id in `<folder>` (default: a new temporary
//! folder): run it again on the same clips with the same folder and nothing is sent, at $0. The scene log's small copy
//! needs a Mac (VideoToolbox).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use klipolo_app::analysis::{AnalysisEvent, AnalysisState, EngineMedia};
use klipolo_app::credentials::{
    Credentials, Endpoints, META_API_KEY, MemoryStore, RELAY_CODE, SecretStore, device_id,
};
use klipolo_engine::audio::pcm::PcmStore;
use klipolo_engine::render::probe::{MediaKind, probe_media};
use klipolo_meta::SecretString;
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::speech::WordKind;
use klipolo_model::time::secs_to_us;

const USAGE: &str = "usage: analyze <clip>... [--cache <folder>]";

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("analyze: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("analyze: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let (clips, folder) = parse()?;
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let (credentials, paying) = credentials(&folder)?;
    let assets = clips
        .iter()
        .enumerate()
        .map(|(i, clip)| asset(i, clip))
        .collect::<Result<Vec<_>, _>>()?;
    let state = AnalysisState::new(
        credentials,
        Arc::new(EngineMedia::new(Some(PcmStore::new(folder.join("audio"))))),
        folder.join("analysis"),
        folder.join("work"),
        Arc::new(|event: AnalysisEvent| {
            let line = serde_json::to_string(&event).unwrap_or_default();
            println!("  {} {line}", event.name());
        }),
        Arc::new(|_| {}),
    );
    println!("paying with: {paying}");
    println!("cache: {}", folder.display());

    let estimate = state.estimate(&assets).await;
    println!(
        "estimate: ${:.4} for {:.1} s of {} clip(s); cached: {}",
        estimate.usd,
        estimate.seconds,
        estimate.clips.len(),
        estimate.cached.len()
    );
    if let Some(left) = estimate.credit_left_usd {
        println!("  credit left: ${left:.2}");
    }
    if let Some(refusal) = &estimate.refusal {
        println!("  refused: {refusal}");
    }

    let started = state.start(&assets).await;
    if let Some(reason) = &started.refused {
        println!("not started: {reason}");
    }
    started.finished().await;

    let mut spent = 0.0;
    for asset in &assets {
        println!(
            "{} ({}): {:?}",
            asset.name,
            asset.id,
            state.state_of(&asset.id)
        );
        let Some(found) = state.analysis_for(&asset.id) else {
            continue;
        };
        spent += found.spent_usd;
        if let Some(t) = &found.transcript {
            let count = |kind| t.words.iter().filter(|w| w.kind == kind).count();
            println!(
                "  words: {}, fillers: {}, pauses: {}, turns: {}, approximate: {}",
                count(WordKind::Word),
                count(WordKind::Filler),
                count(WordKind::Pause),
                found.turns.len(),
                found.approximate
            );
        }
        if let Some(log) = &found.scenes {
            println!(
                "  scene log: {} item(s), {} cut(s)",
                log.items.len(),
                log.cuts.len()
            );
            for item in log.items.iter().take(5) {
                println!(
                    "    {:7.2} to {:7.2} s {:?}: {}",
                    item.start_us as f64 / 1e6,
                    item.end_us as f64 / 1e6,
                    item.kind,
                    item.text
                );
            }
        }
        println!("  cost this run: ${:.6}", found.spent_usd);
    }
    println!("total cost this run: ${spent:.6}");
    Ok(())
}

fn parse() -> Result<(Vec<PathBuf>, PathBuf), String> {
    let mut clips = Vec::new();
    let mut cache = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cache" => cache = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            _ if arg.starts_with("--") => return Err(USAGE.into()),
            _ => clips.push(PathBuf::from(arg)),
        }
    }
    if clips.is_empty() {
        return Err(USAGE.into());
    }
    let folder = cache.unwrap_or_else(|| {
        std::env::temp_dir().join(format!("klipolo-analyze-{}", std::process::id()))
    });
    Ok((clips, folder))
}

/// The credentials the environment gives, in memory, and what pays (never the secret).
fn credentials(folder: &Path) -> Result<(Arc<Credentials>, &'static str), String> {
    let var = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let store = MemoryStore::default();
    let paying = match (var("MODEL_API_KEY"), var("KLIPOLO_ACCESS_CODE")) {
        (Some(key), _) => {
            store
                .set(META_API_KEY, &SecretString::from(key))
                .map_err(|e| e.to_string())?;
            "a Meta key"
        }
        (None, Some(code)) => {
            store
                .set(RELAY_CODE, &SecretString::from(code))
                .map_err(|e| e.to_string())?;
            "an access code, through the relay"
        }
        (None, None) => return Err("Set MODEL_API_KEY or KLIPOLO_ACCESS_CODE.".into()),
    };
    let device = device_id(folder).map_err(|e| e.to_string())?;
    Ok((
        Arc::new(Credentials::load(
            Arc::new(store),
            Endpoints::from_env(),
            device,
        )),
        paying,
    ))
}

/// The clip at `path` as an imported asset.
fn asset(index: usize, path: &Path) -> Result<Asset, String> {
    let path = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let info = probe_media(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let kind = match info.kind {
        MediaKind::Video => AssetKind::Video,
        MediaKind::Audio => AssetKind::Audio,
        MediaKind::Image => return Err(format!("{} is a picture", path.display())),
    };
    Ok(Asset {
        id: format!("clip-{}", index + 1),
        kind,
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.to_string_lossy().into_owned(),
        duration_us: secs_to_us(info.duration),
        width: info.width,
        height: info.height,
        fps: info.fps,
        rotation: (kind == AssetKind::Video).then_some(info.rotation),
        has_audio: info.has_audio,
        audio_lead_us: (kind == AssetKind::Video).then_some(info.audio_lead_us),
    })
}
