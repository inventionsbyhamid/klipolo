//! The timeline's sound lanes mute and set the volume of a kind of recorded sound, the Sounds popover's Centre switch
//! centres it and its Fill gaps switch fills a camera's gaps, with exactly the ops chat's source tools compile to
//! (`muteSource`, `setSourceVolume`, with `which` and `videoSound`) and the rules beside them (`centre_ops`,
//! `fill_gaps_ops`). Rust is the truth: `apps/desktop/src/features/timeline/keys/source-mix-parity.json` holds each
//! request on its project (two takes, a take with two mics, two synced interviews and a take), with the ops written
//! here, and the page's tests check the lanes, the card and the switches against them. With `UPDATE_GOLDEN=1` this test
//! writes the ops again; otherwise the file must be what it writes.
//!
//! The projects are the requests' inputs, kept in the file as the page reads them; each must still load as the model
//! reads it.

use std::path::PathBuf;

use klipolo_chat::compile::{centre_ops, compile, fill_gaps_ops};
use klipolo_chat::tools::Sound;
use klipolo_chat::{FunctionCall, Inputs, Refs, ToolCall};
use klipolo_model::Project;
use klipolo_model::ops::Op;
use serde_json::{Value, json};

fn path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src/features/timeline/keys/source-mix-parity.json")
}

fn project(part: &Value) -> Project {
    serde_json::from_value(part["project"].clone()).expect("a project the model reads")
}

fn sound(word: &Value) -> Sound {
    match word.as_str() {
        Some("mic") => Sound::Mic,
        Some("computerSound") => Sound::ComputerSound,
        Some("gameSound") => Sound::GameSound,
        Some("videoSound") => Sound::VideoSound,
        other => panic!("no sound {other:?}"),
    }
}

/// What chat compiles `call` (`{name, args}`) to on `p`: the additions' ops, or none.
fn chat_ops(p: &Project, call: &Value) -> Vec<Op> {
    let parsed = ToolCall::parse(&FunctionCall {
        name: call["name"].as_str().unwrap().into(),
        call_id: "call_1".into(),
        arguments: call["args"].to_string(),
    })
    .unwrap_or_else(|e| panic!("{call}: {e}"));
    let inputs = Inputs {
        refs: Refs::of(p),
        ..Inputs::default()
    };
    let out = compile(p, &inputs, &[parsed]);
    assert_eq!(out.refusals, vec![], "{call}");
    out.additions.map(|tx| tx.ops).unwrap_or_default()
}

/// The ops for one pinned request on `p`, with the loudness `peaks` the app knew (`[{assetId, peaks}]`).
fn ops_for(p: &Project, call: &Value, peaks: &Value) -> Value {
    let args = &call["args"];
    let ops = match call["name"].as_str() {
        Some("muteSource" | "setSourceVolume") => chat_ops(p, call),
        Some("centreSource") => {
            let known: Vec<(String, Vec<f32>)> = (peaks.as_array().into_iter().flatten())
                .map(|f| {
                    let file = f["assetId"].as_str().unwrap().to_owned();
                    let levels = f["peaks"].as_array().unwrap();
                    (
                        file,
                        levels.iter().map(|v| v.as_f64().unwrap() as f32).collect(),
                    )
                })
                .collect();
            let known: Vec<(&str, &[f32])> = known
                .iter()
                .map(|(f, p)| (f.as_str(), p.as_slice()))
                .collect();
            let centred = args["centred"].as_bool().unwrap();
            centre_ops(p, sound(&args["source"]), centred, &known).expect("every side is known")
        }
        Some("fillGaps") => fill_gaps_ops(
            p,
            args["recording"].as_str().unwrap(),
            args["on"].as_bool().unwrap(),
        ),
        other => panic!("no request {other:?}"),
    };
    serde_json::to_value(ops).unwrap()
}

/// Keys in sorted order, so the file reads the same whether or not a build keeps JSON objects in insertion order.
fn sorted(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(k, v)| (k, sorted(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// `part`'s cases with their ops written again on `part`'s project.
fn cases_again(part: &mut Value) {
    let p = project(part);
    for case in part["cases"].as_array_mut().unwrap() {
        let peaks = case.get("peaks").cloned().unwrap_or(Value::Null);
        case["ops"] = ops_for(&p, &case["call"], &peaks);
    }
}

#[test]
fn the_lanes_and_switches_mix_a_kind_of_sound_as_chat_does() {
    let stored =
        std::fs::read_to_string(path()).unwrap_or_else(|e| panic!("{}: {e}", path().display()));
    let mut golden: Value = serde_json::from_str(&stored).expect("the golden is JSON");
    cases_again(&mut golden);
    cases_again(&mut golden["twoMics"]);
    let camera = &mut golden["cameraSound"];
    cases_again(camera);
    let p = project(camera);
    for name in ["fill", "fillOn"] {
        let call = camera[name]["call"].clone();
        camera[name]["ops"] = ops_for(&p, &call, &Value::Null);
    }
    let muted = &mut camera["fillMuted"];
    let p = project(muted);
    let call = muted["call"].clone();
    muted["ops"] = ops_for(&p, &call, &Value::Null);

    // What the page reads from them, said here too: the mute everywhere reaches both takes' mics and c2's own 60 % one,
    // and leaves c1, whose mic is on its own track; centring the mic plays r1's from its second side.
    let cases = golden["cases"].as_array().unwrap();
    assert_eq!(
        cases[0]["ops"].as_array().unwrap().len(),
        3,
        "{:#}",
        cases[0]
    );
    assert_eq!(cases[5]["ops"][0]["patch"]["mix"]["centreFrom"], 1);
    // Lane n of the mics is `which: n`: each mutes its own mic alone.
    for (case, source) in golden["twoMics"]["cases"]
        .as_array()
        .unwrap()
        .iter()
        .zip(["mic", "mic-2"])
    {
        let ops = case["ops"].as_array().unwrap();
        assert_eq!(ops.len(), 1, "{ops:#?}");
        assert_eq!(ops[0]["sourceId"], source);
    }
    // The camera's sound is the synced interviews', never the take's silent camera.
    for case in golden["cameraSound"]["cases"].as_array().unwrap() {
        let ops = case["ops"].as_array().unwrap();
        assert!(!ops.is_empty(), "{case:#}");
        assert!(
            !ops.iter()
                .any(|op| op["recordingId"] == "r1" || op["clipId"] == "c9"),
            "{case:#}"
        );
    }

    let written = serde_json::to_string_pretty(&sorted(golden.clone())).unwrap() + "\n";
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(path(), &written).unwrap();
    }
    assert!(
        stored == written,
        "chat's source mix changed: run with UPDATE_GOLDEN=1 to write {} again",
        path().display()
    );
    // Every case's request is one of the kinds written here.
    let calls: Vec<&str> = (cases.iter())
        .map(|c| c["call"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        json!(calls),
        json!([
            "muteSource",
            "muteSource",
            "setSourceVolume",
            "muteSource",
            "setSourceVolume",
            "centreSource",
            "centreSource",
            "centreSource"
        ])
    );
}
