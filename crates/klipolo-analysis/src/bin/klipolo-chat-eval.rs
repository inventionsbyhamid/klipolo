//! A development tool for the chat planner's eval against the real API, never part of the app: `klipolo-chat-eval`
//! sends each case of klipolo-chat's `evals/cases.json` to Muse Spark through the chat adapter (`MetaChat`) and the
//! planner, prints pass or fail per case and the total cost.
//!
//! ```text
//! klipolo-chat-eval [--cases <cases.json>] [--only <id>]... [--min-pass <n>]
//! ```
//!
//! It calls Meta for real with `MODEL_API_KEY` (a Meta key) from the environment, never from a file, and never prints
//! it. `KLIPOLO_SPEND_LEDGER` (a CSV file), when set, gets one row per model call; `KLIPOLO_META_BASE` points the
//! calls elsewhere. It exits 0 when at least `--min-pass` cases pass (16 by default, or every case run with `--only`),
//! 1 when fewer do, 2 when it could not run.
//!
//! The eval set names a project, its one asset and that asset's transcript, speaker turns and scene log (paths beside
//! `cases.json`). Every file counts as analyzed, and its loudness is made from the transcript: speech where a word is
//! said, quiet elsewhere, so pause removal finds the transcript's pauses. Each case is one message on that project, on
//! the conversation it gives, with the playhead at 0 and no selection unless it says otherwise. The library the media
//! tools name is the set's own `library` files (as if just imported: not in the project yet) and Klipolo's pack as its
//! `pack` manifest lists it, each pack file where the manifest says.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use klipolo_analysis::providers::meta_chat::MetaChat;
use klipolo_chat::{
    AnalysisState, Audience, ChatError, Inputs, Library, LibraryItem, MediaKind, PlanOut, SceneLog,
    SpeakerTurn, ToolCall, Turn, View, build_context, plan,
};
use klipolo_meta::{MetaClient, SecretString};
use klipolo_model::Project;
use klipolo_model::load_project;
use klipolo_model::project::Asset;
use klipolo_model::speech::{LEVEL_WINDOW_US, Levels, Speech, Transcript, WordKind};
use klipolo_model::time::{RangeUs, Us};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const KEY_ENV: &str = "MODEL_API_KEY";
const USAGE: &str =
    "usage: klipolo-chat-eval [--cases <cases.json>] [--only <id>]... [--min-pass <n>]";
/// The planner's target: at least this many of the 23 cases.
const MIN_PASS: usize = 21;
/// Loudness of a window with a word said in it, and of one without, in dBFS.
const SPEECH_DB: f32 = -20.0;
const QUIET_DB: f32 = -70.0;

/// The eval set: the project and its analysis (paths relative to the set's file), and the cases.
#[derive(Deserialize)]
struct EvalSet {
    project: String,
    asset: String,
    transcript: String,
    turns: String,
    scenes: String,
    /// Klipolo's pack manifest (`assets/library/manifest.json`).
    #[serde(default)]
    pack: Option<String>,
    /// The person's own files in the library.
    #[serde(default)]
    library: Vec<OwnFile>,
    cases: Vec<Case>,
}

/// A file of the person's in the library, made up: a name, what it is, and how long it plays.
#[derive(Deserialize)]
struct OwnFile {
    name: String,
    /// "music", "sound" or "image".
    kind: String,
    #[serde(default)]
    seconds: f64,
}

/// The pack's manifest, as far as the library reads it.
#[derive(Deserialize)]
struct Manifest {
    items: Vec<PackItem>,
}

#[derive(Deserialize)]
struct PackItem {
    id: String,
    name: String,
    /// "music", "sound" or "sticker".
    category: String,
    /// Beside the manifest.
    file: String,
    #[serde(default)]
    duration_ms: Option<u64>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

/// One message, and what it should come to.
#[derive(Deserialize)]
struct Case {
    id: String,
    message: String,
    #[serde(default)]
    history: Vec<Turn>,
    /// Timeline seconds.
    #[serde(default)]
    playhead: f64,
    /// [start, end] in timeline seconds.
    #[serde(default)]
    selection: Option<[f64; 2]>,
    /// Alternatives: the case passes when one of them matches.
    expect: Vec<Expect>,
}

#[derive(Deserialize)]
struct Expect {
    calls: Vec<ExpectedCall>,
    #[serde(default)]
    outcome: Outcome,
}

#[derive(Deserialize)]
struct ExpectedCall {
    name: String,
    /// Only the arguments given are checked (`value_matches`).
    #[serde(default)]
    args: Map<String, Value>,
}

/// Flags on what the message compiled to; one left out isn't checked.
#[derive(Default, Deserialize)]
struct Outcome {
    applied: Option<bool>,
    proposed: Option<bool>,
    asked: Option<bool>,
    undo: Option<bool>,
    jobs: Option<bool>,
}

struct Args {
    cases: PathBuf,
    only: Vec<String>,
    min_pass: Option<usize>,
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
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("klipolo-chat-eval: {e}");
            ExitCode::from(2)
        }
    }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = Args {
        cases: PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../klipolo-chat/evals/cases.json"
        )),
        only: Vec::new(),
        min_pass: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cases" => out.cases = args.next().map(PathBuf::from).ok_or(USAGE)?,
            "--only" => out.only.push(args.next().ok_or(USAGE)?),
            "--min-pass" => {
                let n = args.next().ok_or(USAGE)?;
                out.min_pass = Some(n.parse().map_err(|_| USAGE.to_owned())?);
            }
            _ => return Err(format!("unknown argument {arg}\n{USAGE}")),
        }
    }
    Ok(out)
}

/// The project and its analysis, as every case reads them.
struct Fixture {
    project: Project,
    asset: String,
    speech: Speech,
    turns: HashMap<String, Vec<SpeakerTurn>>,
    scenes: HashMap<String, SceneLog>,
}

fn read(dir: &Path, name: &str) -> Result<String, String> {
    let path = dir.join(name);
    std::fs::read_to_string(&path).map_err(|e| format!("Could not read {}: {e}", path.display()))
}

fn json<T: serde::de::DeserializeOwned>(dir: &Path, name: &str) -> Result<T, String> {
    serde_json::from_str(&read(dir, name)?).map_err(|e| format!("{name}: {e}"))
}

fn fixture(dir: &Path, set: &EvalSet) -> Result<Fixture, String> {
    let project =
        load_project(&read(dir, &set.project)?).map_err(|e| format!("{}: {e}", set.project))?;
    let transcript: Transcript = json(dir, &set.transcript)?;
    let length = (project.asset(&set.asset))
        .ok_or_else(|| format!("{} has no asset {}", set.project, set.asset))?
        .duration_us;
    let mut speech = Speech::default();
    (speech.levels).insert(set.asset.clone(), Arc::new(levels(&transcript, length)));
    (speech.transcripts).insert(set.asset.clone(), Arc::new(transcript));
    Ok(Fixture {
        asset: set.asset.clone(),
        turns: HashMap::from([(set.asset.clone(), json(dir, &set.turns)?)]),
        scenes: HashMap::from([(set.asset.clone(), json(dir, &set.scenes)?)]),
        project,
        speech,
    })
}

/// A library file's asset: its JSON read, so a field added later keeps its default.
fn library_asset(
    id: &str,
    kind: MediaKind,
    path: &str,
    name: &str,
    length: Us,
    size: (u32, u32),
) -> Result<Asset, String> {
    let picture = kind == MediaKind::Image;
    serde_json::from_value(json!({
        "id": id,
        "kind": if picture { "image" } else { "audio" },
        "path": path,
        "name": name,
        "durationUs": length,
        "width": size.0,
        "height": size.1,
        "fps": 0.0,
        "hasAudio": !picture,
    }))
    .map_err(|e| format!("{name}: {e}"))
}

/// The set's own library files, then the pack's items.
fn library(dir: &Path, set: &EvalSet) -> Result<Library, String> {
    let mut items = Vec::new();
    for (i, own) in set.library.iter().enumerate() {
        let kind = match own.kind.as_str() {
            "music" => MediaKind::Music,
            "sound" => MediaKind::Sound,
            "image" => MediaKind::Image,
            other => return Err(format!("{}: no kind {other:?}", own.name)),
        };
        let path = format!("/library/{}", own.name);
        let length = secs_to_us(own.seconds.max(1.0));
        let id = format!("library-{i}");
        let asset = library_asset(&id, kind, &path, &own.name, length, (1024, 1024))?;
        items.push(LibraryItem {
            name: own.name.clone(),
            kind,
            asset,
            pack: false,
        });
    }
    if let Some(manifest) = &set.pack {
        let manifest_path = dir.join(manifest);
        let folder = manifest_path.parent().unwrap_or(dir).to_path_buf();
        let read: Manifest = json(dir, manifest)?;
        for it in read.items {
            let kind = match it.category.as_str() {
                "music" => MediaKind::Music,
                "sound" => MediaKind::Sound,
                _ => MediaKind::Image,
            };
            let length = it.duration_ms.map_or(5_000_000, |ms| ms as Us * 1000);
            let size = (it.width.unwrap_or(0), it.height.unwrap_or(0));
            let path = folder.join(&it.file).to_string_lossy().into_owned();
            let id = format!("pack-{}", it.id);
            let asset = library_asset(&id, kind, &path, &it.name, length, size)?;
            items.push(LibraryItem {
                name: it.name,
                kind,
                asset,
                pack: true,
            });
        }
    }
    Ok(Library { items })
}

/// Loudness for `length` of sound from `transcript`: a window is speech when a word (not a pause) is said in it.
fn levels(transcript: &Transcript, length: Us) -> Levels {
    let windows = (length.max(0) as usize).div_ceil(LEVEL_WINDOW_US as usize);
    let mut db = vec![QUIET_DB; windows];
    for w in (transcript.words.iter()).filter(|w| w.kind != WordKind::Pause) {
        let first = (w.start_us.max(0) / LEVEL_WINDOW_US) as usize;
        let last = ((w.end_us.max(0) + LEVEL_WINDOW_US - 1) / LEVEL_WINDOW_US) as usize;
        for level in db.iter_mut().take(last).skip(first) {
            *level = SPEECH_DB;
        }
    }
    Levels { db: db.into() }
}

fn secs_to_us(s: f64) -> Us {
    (s * 1e6).round() as Us
}

fn run(args: &Args) -> Result<bool, String> {
    let set: EvalSet = {
        let text = std::fs::read_to_string(&args.cases)
            .map_err(|e| format!("Could not read {}: {e}", args.cases.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", args.cases.display()))?
    };
    let dir = args.cases.parent().unwrap_or(Path::new("."));
    let fx = fixture(dir, &set)?;
    let library = library(dir, &set)?;
    let cases: Vec<&Case> = (set.cases.iter())
        .filter(|c| args.only.is_empty() || args.only.contains(&c.id))
        .collect();
    if cases.is_empty() {
        return Err("No case to run.".into());
    }
    let key = std::env::var(KEY_ENV)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("Set {KEY_ENV}."))?;
    let model = MetaChat::new(MetaClient::direct(SecretString::from(key)));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Could not start: {e}"))?;

    let (mut passed, mut cost, mut model_calls) = (0, 0.0, 0);
    for case in &cases {
        let view = View {
            playhead_us: secs_to_us(case.playhead),
            selection: case.selection.map(|[a, b]| RangeUs {
                start: secs_to_us(a),
                end: secs_to_us(b),
            }),
        };
        let ctx = build_context(
            &fx.project,
            &fx.speech,
            &fx.turns,
            &fx.scenes,
            &view,
            &library,
        );
        let inputs = Inputs {
            message: case.message.clone(),
            view,
            refs: ctx.refs.clone(),
            speech: fx.speech.clone(),
            scenes: fx.scenes.clone(),
            analysis: HashMap::from([(fx.asset.clone(), AnalysisState::Done)]),
            library: library.clone(),
        };
        let planned = runtime.block_on(plan(
            &model,
            &case.history,
            &ctx,
            &case.message,
            &inputs,
            &fx.project,
        ));
        let (verdict, spent, calls_made) = match &planned {
            Ok(out) => (judge(case, out), out.usage_usd, out.answers.len()),
            Err(e) => (Err(failed(e)), e.usage_usd, e.model_calls),
        };
        cost += spent;
        model_calls += calls_made;
        let mark = if verdict.is_ok() { "PASS" } else { "FAIL" };
        passed += usize::from(verdict.is_ok());
        println!(
            "{mark} {:<18} ${spent:.5}  {}",
            case.id,
            match &planned {
                Ok(out) => shown_answers(out),
                Err(_) => "no answer".to_owned(),
            }
        );
        match verdict {
            Err(why) => println!("     why: {why}"),
            // Which way it passed, when the case allows more than one.
            Ok(k) if case.expect.len() > 1 => {
                println!("     passed on alternative #{k} of {}", case.expect.len());
            }
            Ok(_) => {}
        }
        if let Ok(out) = &planned {
            println!("     reply: {}", out.reply);
        }
    }
    let min = (args.min_pass)
        .unwrap_or(if args.only.is_empty() {
            MIN_PASS
        } else {
            cases.len()
        })
        .min(cases.len());
    println!(
        "{passed}/{} passed (target {min}); {model_calls} model calls; total ${cost:.4}",
        cases.len()
    );
    Ok(passed >= min)
}

fn failed(e: &ChatError) -> String {
    format!("{:?}: {} ({})", e.kind, e.message, e.detail)
}

/// Each answer's calls as the model wrote them: `name(args) | name(args)`, answers apart with `then`.
fn shown_answers(out: &PlanOut) -> String {
    let answers: Vec<String> = (out.answers.iter())
        .map(|calls| {
            if calls.is_empty() {
                return "(no call)".to_owned();
            }
            (calls.iter())
                .map(|c| format!("{}({})", c.name, c.arguments))
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .collect();
    answers.join(" then ")
}

/// The first of the case's alternatives that `out` matches, by index, else why none does.
fn judge(case: &Case, out: &PlanOut) -> Result<usize, String> {
    let refused: Vec<&str> = (out.compiled.refusals.iter())
        .filter(|r| r.audience == Audience::Model)
        .map(|r| r.reason.as_str())
        .collect();
    if !refused.is_empty() {
        return Err(format!("still refused: {}", refused.join(" ")));
    }
    // The model's first answer as it wrote it, and after a refusal round the calls compiled.
    let mut candidates: Vec<Vec<(String, Value)>> = vec![
        (out.answers.first().into_iter().flatten())
            .map(|c| {
                let args = serde_json::from_str(&c.arguments).unwrap_or(Value::Null);
                (c.name.clone(), args)
            })
            .collect(),
    ];
    if out.answers.len() > 1 {
        candidates.push(out.calls.iter().map(tool_json).collect());
    }
    let mut why = Vec::new();
    for (k, expect) in case.expect.iter().enumerate() {
        if !candidates.iter().any(|c| calls_match(&expect.calls, c)) {
            // Said against the calls compiled last: the first answer's, or the refusal round's.
            let last = candidates.last().map(Vec::as_slice).unwrap_or_default();
            why.push(format!("#{k}: {}", mismatch(&expect.calls, last)));
            continue;
        }
        match outcome_fails(&expect.outcome, out) {
            None => return Ok(k),
            Some(flag) => why.push(format!("#{k}: {flag}")),
        }
    }
    Err(why.join("; "))
}

/// The first outcome flag `out` doesn't meet.
fn outcome_fails(o: &Outcome, out: &PlanOut) -> Option<String> {
    let c = &out.compiled;
    let flags = [
        ("applied", o.applied, c.additions.is_some()),
        ("proposed", o.proposed, c.proposal.is_some()),
        ("asked", o.asked, !c.questions.is_empty()),
        ("undo", o.undo, c.undo),
        ("jobs", o.jobs, !c.jobs.is_empty()),
    ];
    (flags.iter())
        .find(|(_, want, got)| want.is_some_and(|w| w != *got))
        .map(|(name, want, _)| format!("{name} should be {}", want.unwrap_or_default()))
}

/// Whether `actual` holds exactly the tools `expected` names (in any order), each with arguments that match.
fn calls_match(expected: &[ExpectedCall], actual: &[(String, Value)]) -> bool {
    fn assign(expected: &[ExpectedCall], actual: &[(String, Value)], used: &mut [bool]) -> bool {
        let Some((first, rest)) = expected.split_first() else {
            return true;
        };
        for (i, (name, args)) in actual.iter().enumerate() {
            if used[i] || *name != first.name {
                continue;
            }
            let fits = (first.args.iter())
                .all(|(k, want)| args.get(k).is_some_and(|got| value_matches(want, got)));
            if fits {
                used[i] = true;
                if assign(rest, actual, used) {
                    return true;
                }
                used[i] = false;
            }
        }
        false
    }
    expected.len() == actual.len() && assign(expected, actual, &mut vec![false; actual.len()])
}

/// Why `actual` doesn't match `expected`: other tools, or the first argument of a tool that doesn't fit.
fn mismatch(expected: &[ExpectedCall], actual: &[(String, Value)]) -> String {
    let mut want: Vec<&str> = expected.iter().map(|c| c.name.as_str()).collect();
    let mut got: Vec<&str> = actual.iter().map(|(name, _)| name.as_str()).collect();
    want.sort_unstable();
    got.sort_unstable();
    if want != got {
        let list = |names: &[&str]| match names {
            [] => "no call".to_owned(),
            names => names.join(" and "),
        };
        return format!("{} instead of {}", list(&got), list(&want));
    }
    for e in expected {
        for (key, rule) in &e.args {
            let wrong = (actual.iter())
                .filter(|(name, _)| *name == e.name)
                .map(|(_, args)| args.get(key))
                .find(|got| !got.is_some_and(|got| value_matches(rule, got)));
            if let Some(got) = wrong {
                let got = got.map_or("missing".to_owned(), Value::to_string);
                return format!("{}.{key} is {got}, not {rule}", e.name);
            }
        }
    }
    "the calls don't pair up with the expected ones".to_owned()
}

/// Whether `got` matches `want`: a number within 0.01, `{min, max}` a range, a string ignoring case, `{contains}` a
/// part of it, `{anyOf}` one of several, `{minItems}` a list of at least that many items, a list item by item, anything
/// else equal.
fn value_matches(want: &Value, got: &Value) -> bool {
    match want {
        Value::Number(n) => {
            (n.as_f64().zip(got.as_f64())).is_some_and(|(w, g)| (w - g).abs() <= 0.01)
        }
        Value::String(w) => got.as_str().is_some_and(|g| g.eq_ignore_ascii_case(w)),
        Value::Array(items) => got.as_array().is_some_and(|g| {
            g.len() == items.len() && items.iter().zip(g).all(|(w, g)| value_matches(w, g))
        }),
        Value::Object(rule) if rule.contains_key("min") || rule.contains_key("max") => {
            got.as_f64().is_some_and(|g| {
                let min = rule.get("min").and_then(Value::as_f64).unwrap_or(f64::MIN);
                let max = rule.get("max").and_then(Value::as_f64).unwrap_or(f64::MAX);
                (min..=max).contains(&g)
            })
        }
        Value::Object(rule) if rule.contains_key("contains") => {
            let part = rule["contains"].as_str().unwrap_or_default().to_lowercase();
            got.as_str()
                .is_some_and(|g| g.to_lowercase().contains(&part))
        }
        Value::Object(rule) if rule.contains_key("anyOf") => (rule["anyOf"].as_array())
            .is_some_and(|options| options.iter().any(|w| value_matches(w, got))),
        Value::Object(rule) if rule.contains_key("minItems") => {
            let least = rule["minItems"].as_u64().unwrap_or(0) as usize;
            got.as_array().is_some_and(|g| g.len() >= least)
        }
        Value::Object(fields) => got.as_object().is_some_and(|g| {
            (fields.iter()).all(|(k, w)| g.get(k).is_some_and(|got| value_matches(w, got)))
        }),
        other => other == got,
    }
}

fn s(us: Us) -> f64 {
    us as f64 / 1e6
}

fn range_json(r: RangeUs) -> Value {
    json!({"startSec": s(r.start), "endSec": s(r.end)})
}

/// A compiled call as the model would have written it: its name and its arguments in seconds and words.
fn tool_json(call: &ToolCall) -> (String, Value) {
    let args = match call {
        ToolCall::CutRange { range } => range_json(*range),
        ToolCall::KeepOnly { ranges } => {
            json!({"ranges": ranges.iter().map(|r| range_json(*r)).collect::<Vec<_>>()})
        }
        ToolCall::RemoveSilences { min_pause_us } => {
            json!({"minPauseSec": min_pause_us.map(s)})
        }
        ToolCall::RemoveFillers | ToolCall::Undo => json!({}),
        ToolCall::TrimClip { clip, edge, to_us } => {
            json!({"clip": clip, "edge": word(edge), "toSec": s(*to_us)})
        }
        ToolCall::SplitAt { at_us } => json!({"atSec": s(*at_us)}),
        ToolCall::SetFormat {
            format,
            keep_others,
        } => json!({"format": word(format), "keepOthers": keep_others}),
        ToolCall::SetCaptions { on, style } => {
            json!({"on": on, "style": style.map(|st| st.key())})
        }
        ToolCall::FixCaptionWords { find, replace } => json!({"find": find, "replace": replace}),
        ToolCall::FindMoments { query, moments } => json!({
            "query": query,
            "ranges": moments.iter().map(|m| {
                let mut r = range_json(m.range);
                r["label"] = json!(m.label);
                r
            }).collect::<Vec<_>>(),
        }),
        ToolCall::FollowSubject { description } | ToolCall::BlurObjects { description } => {
            json!({"description": description})
        }
        ToolCall::AddMusic {
            file,
            volume,
            duck,
            at_us,
        } => json!({"file": file, "volume": volume, "duck": duck, "atSec": at_us.map(s)}),
        ToolCall::AddSound { name, at_us } => {
            json!({"name": name, "atSec": at_us.iter().map(|&t| s(t)).collect::<Vec<_>>()})
        }
        ToolCall::AddText {
            text,
            preset,
            at_us,
            duration_us,
        } => json!({
            "text": text,
            "preset": preset.key(),
            "atSec": at_us.map(s),
            "durationSec": duration_us.map(s),
        }),
        ToolCall::AddImage {
            file,
            size,
            position,
            at_us,
            duration_us,
        } => json!({
            "file": file,
            "size": size.map(|x| x.key()),
            "position": position.map(|x| x.key()),
            "atSec": at_us.map(s),
            "durationSec": duration_us.map(s),
        }),
        ToolCall::MakeSticker {
            prompt,
            position,
            at_us,
            duration_us,
        } => json!({
            "prompt": prompt,
            "position": position.map(|x| x.key()),
            "atSec": at_us.map(s),
            "durationSec": duration_us.map(s),
        }),
        ToolCall::MakeTitleCard {
            title,
            prompt,
            at_us,
            duration_us,
        } => json!({
            "title": title,
            "prompt": prompt,
            "atSec": at_us.map(s),
            "durationSec": duration_us.map(s),
        }),
        ToolCall::AskUser { question } => json!({"question": question}),
        ToolCall::CannotDo { reason } => json!({"reason": reason}),
    };
    (call.name().to_owned(), args)
}

/// The model's word for a value it writes (`portrait`, `end`): its JSON name.
fn word<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(word)) => word,
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected(name: &str, args: Value) -> ExpectedCall {
        ExpectedCall {
            name: name.into(),
            args: args.as_object().cloned().unwrap_or_default(),
        }
    }

    #[test]
    fn values_match_by_rule() {
        assert!(value_matches(&json!(1.0), &json!(1)));
        assert!(!value_matches(&json!(1.0), &json!(1.5)));
        assert!(value_matches(
            &json!({"min": 12.5, "max": 14.5}),
            &json!(13.5)
        ));
        assert!(!value_matches(
            &json!({"min": 12.5, "max": 14.5}),
            &json!(15)
        ));
        assert!(value_matches(&json!("Portrait"), &json!("portrait")));
        assert!(value_matches(
            &json!({"contains": "face"}),
            &json!("Kids' Faces")
        ));
        assert!(value_matches(
            &json!({"anyOf": ["karaoke", "highlight"]}),
            &json!("highlight")
        ));
        assert!(!value_matches(
            &json!({"anyOf": ["karaoke"]}),
            &json!("bold")
        ));
        assert!(value_matches(&json!(true), &json!(true)));
        assert!(!value_matches(&json!(true), &json!(null)));
        // A list of at least so many items.
        assert!(value_matches(&json!({"minItems": 3}), &json!([1, 2, 3])));
        assert!(!value_matches(&json!({"minItems": 3}), &json!([1, 2])));
        assert!(!value_matches(&json!({"minItems": 1}), &json!("a")));
        let range = json!([{"startSec": {"min": 63.0, "max": 68.0}}]);
        assert!(value_matches(
            &range,
            &json!([{"startSec": 67.0, "endSec": 85.0}])
        ));
        assert!(!value_matches(
            &range,
            &json!([{"startSec": 67.0}, {"startSec": 70.0}])
        ));
    }

    #[test]
    fn calls_match_as_a_set_of_tools() {
        let want = [
            expected("setFormat", json!({"format": "square"})),
            expected("removeSilences", json!({"minPauseSec": 1.0})),
        ];
        let got = |pairs: &[(&str, Value)]| -> Vec<(String, Value)> {
            pairs
                .iter()
                .map(|(n, a)| (n.to_string(), a.clone()))
                .collect()
        };
        let both = got(&[
            ("removeSilences", json!({"minPauseSec": 1})),
            ("setFormat", json!({"format": "square", "keepOthers": true})),
        ]);
        assert!(calls_match(&want, &both));
        // An extra call, a missing one, or a wrong argument fails.
        let mut extra = both.clone();
        extra.push(("followSubject".into(), json!({"description": "speaker"})));
        assert!(!calls_match(&want, &extra));
        assert!(!calls_match(&want, &both[..1]));
        let wrong = got(&[
            ("removeSilences", json!({"minPauseSec": 2})),
            ("setFormat", json!({"format": "square"})),
        ]);
        assert!(!calls_match(&want, &wrong));
        // A name alone matches any arguments.
        assert!(calls_match(
            &[expected("askUser", json!({}))],
            &got(&[("askUser", json!({"question": "Which part?"}))])
        ));
    }

    #[test]
    fn a_mismatch_names_the_tools_or_the_argument() {
        let want = [expected(
            "cutRange",
            json!({"startSec": {"min": 85.0, "max": 87.0}, "endSec": {"min": 92.3, "max": 97.1}}),
        )];
        let got = |name: &str, args: Value| vec![(name.to_owned(), args)];
        assert_eq!(
            mismatch(
                &want,
                &got("cutRange", json!({"startSec": 85.5, "endSec": 99.3}))
            ),
            r#"cutRange.endSec is 99.3, not {"max":97.1,"min":92.3}"#
        );
        assert_eq!(
            mismatch(&want, &got("cutRange", json!({"startSec": 85.5}))),
            r#"cutRange.endSec is missing, not {"max":97.1,"min":92.3}"#
        );
        assert_eq!(
            mismatch(&want, &got("askUser", json!({}))),
            "askUser instead of cutRange"
        );
        assert_eq!(mismatch(&want, &[]), "no call instead of cutRange");
    }

    #[test]
    fn compiled_calls_read_as_the_model_writes_them() {
        use klipolo_chat::FunctionCall;
        let written = [
            ("cutRange", json!({"startSec": 0.0, "endSec": 13.5})),
            (
                "keepOnly",
                json!({"ranges": [{"startSec": 67.0, "endSec": 85.0}]}),
            ),
            ("removeSilences", json!({"minPauseSec": 1.0})),
            (
                "trimClip",
                json!({"clip": "c1", "edge": "end", "toSec": 12.8}),
            ),
            (
                "setFormat",
                json!({"format": "portrait", "keepOthers": true}),
            ),
            ("setCaptions", json!({"on": true, "style": "karaoke"})),
            ("followSubject", json!({"description": "woman astronaut"})),
            (
                "addMusic",
                json!({"file": "lofi", "volume": null, "duck": true, "atSec": 0.0}),
            ),
            ("addSound", json!({"name": "whoosh", "atSec": [12.8, 23.0]})),
            (
                "addText",
                json!({"text": "Day 1", "preset": "title", "atSec": 0.0, "durationSec": 3.0}),
            ),
            (
                "addImage",
                json!({"file": "logo", "size": "small", "position": "top-right", "atSec": null, "durationSec": null}),
            ),
            (
                "makeSticker",
                json!({"prompt": "paper plane", "position": null, "atSec": 67.0, "durationSec": 5.0}),
            ),
            (
                "makeTitleCard",
                json!({"title": "Day 1", "prompt": "trams", "atSec": 0.0, "durationSec": null}),
            ),
        ];
        for (name, args) in written {
            let call = ToolCall::parse(&FunctionCall {
                name: name.into(),
                call_id: "c".into(),
                arguments: args.to_string(),
            })
            .unwrap();
            assert_eq!(tool_json(&call), (name.to_owned(), args));
        }
    }

    #[test]
    fn the_library_is_the_sets_own_files_and_the_pack() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../klipolo-chat/evals");
        let text = std::fs::read_to_string(dir.join("cases.json")).unwrap();
        let set: EvalSet = serde_json::from_str(&text).unwrap();
        let library = library(&dir, &set).unwrap();
        let first = &library.items[0];
        assert_eq!(
            (first.name.as_str(), first.kind, first.pack),
            ("lofi-beat.mp3", MediaKind::Music, false)
        );
        assert!(first.asset.has_audio && first.asset.duration_us > 60_000_000);
        // The pack as its manifest lists it, each file where the manifest says.
        assert_eq!(library.items.len(), 1 + 16);
        let whoosh = (library.items.iter()).find(|i| i.name == "Whoosh").unwrap();
        assert_eq!((whoosh.kind, whoosh.pack), (MediaKind::Sound, true));
        assert_eq!(whoosh.asset.duration_us, 900_000);
        assert!(
            Path::new(&whoosh.asset.path).is_file(),
            "{}",
            whoosh.asset.path
        );
        let heart = (library.items.iter()).find(|i| i.name == "Heart").unwrap();
        assert_eq!(heart.kind, MediaKind::Image);
        assert!(heart.asset.width > 0 && heart.asset.height > 0);
    }

    #[test]
    fn loudness_is_speech_where_words_are_said() {
        let transcript: Transcript = serde_json::from_value(json!({
            "key": "k", "language": "en", "model": "m", "coveredUs": 1_000_000, "complete": true,
            "words": [
                {"text": "Hi", "startUs": 100_000, "endUs": 250_000, "confidence": 1.0, "kind": "word"},
                {"text": "", "startUs": 250_000, "endUs": 950_000, "confidence": 1.0, "kind": "pause"}
            ]
        }))
        .unwrap();
        let l = levels(&transcript, 1_000_000);
        assert_eq!(l.db.len(), 10);
        assert_eq!(&l.db[..4], &[QUIET_DB, SPEECH_DB, SPEECH_DB, QUIET_DB]);
        assert!(l.db[3..].iter().all(|&d| d == QUIET_DB));
    }
}
