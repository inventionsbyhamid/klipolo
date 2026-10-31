//! A development tool for the smoke test against the real API, never part of the app: chats with a project made of one
//! clip through the app's own chat (`klipolo_app::chat::ChatSession`, what the chat panel's messages run), and prints
//! each reply, what it applied, what it proposes, and what it all cost.
//!
//! ```text
//! cargo run -p klipolo-app --example chat -- <clip> "<message>"... [--cache <folder>] [--apply]
//! ```
//!
//! The clip is analysed first by the app's analysis (`--cache`: the folder `examples/analyze` keeps, so a clip analysed
//! before costs nothing; default: a new temporary folder, which pays for its analysis). Each message then goes to the
//! model in turn, as the panel sends them, on the project the one before left. With `--apply`, a proposal is applied,
//! undone and redone, and the project is printed after each step.
//!
//! The person's Meta key comes from `MODEL_API_KEY`, or an access code from `KLIPOLO_ACCESS_CODE`; neither is printed.
//! They are held in memory only (never the Keychain). `KLIPOLO_SPEND_LEDGER` (a CSV file), when set, gets one row per
//! paid call; `KLIPOLO_META_BASE` and `KLIPOLO_RELAY_BASE` point the calls elsewhere.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use klipolo_app::analysis::{AnalysisState, EngineMedia};
use klipolo_app::chat::{ChatOutcome, ChatRequest, ChatSession, MemoryProjects, MetaModels};
use klipolo_app::credentials::{
    Credentials, Endpoints, META_API_KEY, MemoryStore, RELAY_CODE, SecretStore, device_id,
};
use klipolo_engine::audio::pcm::PcmStore;
use klipolo_engine::render::probe::{MediaKind, probe_media};
use klipolo_meta::SecretString;
use klipolo_model::Project;
use klipolo_model::project::{Asset, AssetKind};
use klipolo_model::time::{US_PER_SEC, Us, secs_to_us};
use klipolo_model::timeline::{Clip, ClipSource};
use serde_json::Value;

const USAGE: &str = "usage: chat <clip> \"<message>\"... [--cache <folder>] [--apply]";

struct Args {
    clip: PathBuf,
    messages: Vec<String>,
    folder: PathBuf,
    apply: bool,
}

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("chat: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("chat: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let args = parse()?;
    std::fs::create_dir_all(&args.folder).map_err(|e| format!("{}: {e}", args.folder.display()))?;
    let (credentials, paying) = credentials(&args.folder)?;
    let asset = asset(&args.clip)?;
    println!("paying with: {paying}");
    println!("cache: {}", args.folder.display());

    // The clip's analysis, as Analyze runs it (cached: at once, at $0).
    let analysis = AnalysisState::new(
        credentials.clone(),
        Arc::new(EngineMedia::new(Some(PcmStore::new(
            args.folder.join("audio"),
        )))),
        args.folder.join("analysis"),
        args.folder.join("work"),
        Arc::new(|_| {}),
        Arc::new(|_| {}),
    );
    let estimate = analysis.estimate(std::slice::from_ref(&asset)).await;
    println!(
        "analysis: ${:.4} to pay, {} cached",
        estimate.usd,
        estimate.cached.len()
    );
    let started = analysis.start(std::slice::from_ref(&asset)).await;
    if let Some(reason) = &started.refused {
        return Err(format!("the analysis didn't start: {reason}"));
    }
    started.finished().await;
    let analysed_usd = analysis
        .analysis_for(&asset.id)
        .map_or(0.0, |a| a.spent_usd);
    println!(
        "analysis: {:?}, ${analysed_usd:.4} this run",
        analysis.state_of(&asset.id)
    );

    let project = Arc::new(
        MemoryProjects::open(project_of(asset), args.folder.join("chat.klipolo"))
            .map_err(|e| e.to_string())?,
    );
    let chat = ChatSession::new(
        project.clone(),
        analysis,
        Arc::new(MetaModels::new(credentials)),
    );
    show_project("project", &project.project());

    for (n, text) in args.messages.iter().enumerate() {
        println!();
        println!("> {text}");
        let request: ChatRequest = serde_json::from_value(serde_json::json!({
            "id": format!("message-{}", n + 1),
            "text": text,
            "selection": null,
            "item": null,
            "playheadUs": 0,
            "format": null,
            "baseRevision": project.revision(),
        }))
        .map_err(|e| e.to_string())?;
        let outcome = chat
            .send(request)
            .await
            .map_err(|e| format!("{:?}", e.said()))?;
        let proposal = show_outcome(&outcome);
        show_project("project", &project.project());
        if let (true, Some(id)) = (args.apply, proposal) {
            match chat.apply(&id) {
                Ok(applied) => {
                    println!(
                        "applied: undo {} (revision {} to {})",
                        applied.undo.code,
                        applied.change.base_revision,
                        applied.change.history.revision
                    );
                    show_project("after Apply", &project.project());
                    project.step(false).ok_or("nothing to undo")?;
                    show_project("after Undo", &project.project());
                    project.step(true).ok_or("nothing to redo")?;
                    show_project("after Redo", &project.project());
                }
                Err(e) => println!("apply refused: {:?}", e.said()),
            }
        }
    }

    let spend = chat.spend();
    println!();
    println!(
        "chat: {} message(s), ${:.6}; analysis this run: ${analysed_usd:.6}",
        spend.messages, spend.chat_usd
    );
    Ok(())
}

/// Prints the reply as the panel gets it; the proposal's id, when there is one.
fn show_outcome(outcome: &ChatOutcome) -> Option<String> {
    let v = serde_json::to_value(outcome).unwrap_or_default();
    if v["type"] == "failed" {
        println!("failed ({}): {}", v["kind"], v["message"]);
        return None;
    }
    println!("reply: {}", v["text"]);
    for key in ["notes", "questions"] {
        for line in v[key].as_array().into_iter().flatten() {
            println!("  {key}: {line}");
        }
    }
    for m in v["moments"].as_array().into_iter().flatten() {
        println!(
            "  moment: {} to {} {}",
            seconds(&m["startUs"]),
            seconds(&m["endUs"]),
            m["label"]
        );
    }
    if v["applied"].is_object() {
        let change = &v["applied"]["change"];
        let changed: Vec<&str> = (change["patch"].as_object().into_iter().flatten())
            .filter(|(_, value)| !value.is_null())
            .map(|(key, _)| key.as_str())
            .collect();
        println!(
            "  applied: undo {} {}, revision {} to {}, changed {changed:?}",
            v["applied"]["undo"]["code"],
            v["applied"]["undo"]["params"],
            change["baseRevision"],
            change["history"]["revision"]
        );
    }
    for picture in v["pictures"].as_array().into_iter().flatten() {
        println!(
            "  picture asked for: {} {}",
            picture["kind"], picture["prompt"]
        );
    }
    if v["undone"].is_object() {
        println!("  undone: revision {}", v["undone"]["history"]["revision"]);
    }
    let p = &v["proposal"];
    if !p.is_object() {
        return None;
    }
    println!(
        "  proposal: {} ({} removed, on revision {})",
        p["title"],
        seconds(&p["removedUs"]),
        p["baseRevision"]
    );
    for range in p["removals"]["ranges"].as_array().into_iter().flatten() {
        println!(
            "    cut {} to {}",
            seconds(&range["start"]),
            seconds(&range["end"])
        );
    }
    let others: Vec<String> = (p["removals"].as_object().into_iter().flatten())
        .filter(|(key, value)| *key != "ranges" && value.as_array().is_some_and(|a| !a.is_empty()))
        .map(|(key, value)| format!("{key} {value}"))
        .collect();
    if !others.is_empty() {
        println!("    also removes: {}", others.join(", "));
    }
    let tags: Vec<&str> = (p["tags"].as_array().into_iter().flatten())
        .filter_map(|t| t["label"].as_str())
        .collect();
    if !tags.is_empty() {
        println!("    tags: {}", tags.join(" "));
    }
    p["id"].as_str().map(str::to_owned)
}

/// The project as the smoke checks it: its formats, captions and main track.
fn show_project(what: &str, p: &Project) {
    let clips: Vec<String> = (p.timeline.main.iter())
        .map(|c| format!("{}-{}", secs(c.source_in_us), secs(c.source_out_us)))
        .collect();
    let length: Us = (p.timeline.main.iter())
        .map(|c| c.source_out_us - c.source_in_us)
        .sum();
    println!(
        "{what}: {:?} of {:?}, captions {}, {} s in {} clip(s) [{}]",
        p.settings.primary_format,
        p.settings.formats,
        serde_json::to_string(&p.captions).unwrap_or_default(),
        secs(length),
        clips.len(),
        clips.join(", ")
    );
}

fn secs(us: Us) -> String {
    format!("{:.2}", us as f64 / US_PER_SEC as f64)
}

fn seconds(v: &Value) -> String {
    v.as_i64()
        .map_or_else(|| "?".to_owned(), |us| format!("{} s", secs(us)))
}

fn parse() -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut cache = None;
    let mut apply = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cache" => cache = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            "--apply" => apply = true,
            _ if arg.starts_with("--") => return Err(USAGE.into()),
            _ => positional.push(arg),
        }
    }
    let mut positional = positional.into_iter();
    let clip = PathBuf::from(positional.next().ok_or(USAGE)?);
    let messages: Vec<String> = positional.collect();
    if messages.is_empty() {
        return Err(USAGE.into());
    }
    let folder = cache.unwrap_or_else(|| {
        std::env::temp_dir().join(format!("klipolo-chat-{}", std::process::id()))
    });
    Ok(Args {
        clip,
        messages,
        folder,
        apply,
    })
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

/// The clip at `path` as an imported asset (the same id the analyze example gives it, so its cache is shared).
fn asset(path: &Path) -> Result<Asset, String> {
    let path = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let info = probe_media(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let kind = match info.kind {
        MediaKind::Video => AssetKind::Video,
        MediaKind::Audio => AssetKind::Audio,
        MediaKind::Image => return Err(format!("{} is a picture", path.display())),
    };
    Ok(Asset {
        id: "clip-1".into(),
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
        made_with: None,
    })
}

/// A project of `asset` played whole on the main track.
fn project_of(asset: Asset) -> Project {
    let name = asset.name.clone();
    let mut project = Project::new(if name.is_empty() { "Chat smoke" } else { &name });
    let mut clip = Clip::new(
        ClipSource::Asset {
            id: asset.id.clone(),
        },
        0,
        asset.duration_us,
    );
    clip.id = "clip-1-main".into();
    project.assets = vec![asset];
    project.timeline.main = vec![clip];
    project
}
