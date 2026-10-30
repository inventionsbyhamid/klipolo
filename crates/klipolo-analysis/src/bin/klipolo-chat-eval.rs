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
//! the conversation it gives, with the playhead at 0 and no selection unless it says otherwise.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use klipolo_analysis::providers::meta_chat::MetaChat;
use klipolo_chat::{
    AnalysisState, Audience, ChatError, Inputs, PlanOut, SceneLog, SpeakerTurn, ToolCall, Turn,
    View, build_context, plan,
};
use klipolo_meta::{MetaClient, SecretString};
use klipolo_model::Project;
use klipolo_model::load_project;
use klipolo_model::speech::{LEVEL_WINDOW_US, Levels, Speech, Transcript, WordKind};
use klipolo_model::time::{RangeUs, Us};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const KEY_ENV: &str = "MODEL_API_KEY";
const USAGE: &str =
    "usage: klipolo-chat-eval [--cases <cases.json>] [--only <id>]... [--min-pass <n>]";
/// The planner's target: at least this many of the 18 cases.
const MIN_PASS: usize = 16;
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
    cases: Vec<Case>,
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
        let ctx = build_context(&fx.project, &fx.speech, &fx.turns, &fx.scenes, &view);
        let inputs = Inputs {
            message: case.message.clone(),
            view,
            refs: ctx.refs.clone(),
            speech: fx.speech.clone(),
            scenes: fx.scenes.clone(),
            analysis: HashMap::from([(fx.asset.clone(), AnalysisState::Done)]),
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
            Err(e) => (Err(failed(e)), e.usage_usd, 0),
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
        if let Err(why) = verdict {
            println!("     why: {why}");
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
    format!("{:?}: {}", e.kind, e.message)
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

/// Ok when one of the case's alternatives matches `out`, else why not.
fn judge(case: &Case, out: &PlanOut) -> Result<(), String> {
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
            let names: Vec<&str> = expect.calls.iter().map(|c| c.name.as_str()).collect();
            why.push(format!("#{k}: calls are not {}", names.join(" and ")));
            continue;
        }
        match outcome_fails(&expect.outcome, out) {
            None => return Ok(()),
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

/// Whether `got` matches `want`: a number within 0.01, `{min, max}` a range, a string ignoring case, `{contains}` a
/// part of it, `{anyOf}` one of several, a list item by item, anything else equal.
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
