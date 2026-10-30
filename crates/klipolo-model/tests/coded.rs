mod common;
use std::collections::BTreeMap;
use std::path::PathBuf;

use common::*;
use klipolo_model::coded::{english, CodedError, ErrorParam, Messages, Said};
use klipolo_model::messages::MESSAGES;
use klipolo_model::ops::{Op, Transaction};
use klipolo_model::{apply_ops, Document};
use serde_json::json;

const TEST: Messages = &[
    ("t.cut", "Cut {startUs}–{endUs}"),
    ("t.add", "Add {effect}"),
    ("t.part", "Part of {item}"),
    ("t.text", "Text “{text}”"),
    ("t.count", "{count} clips"),
    ("t.since", "since {sinceAtMs}"),
    ("t.again", "Transcribe it again in {language}"),
    ("t.model", "a {sizeBytes} model"),
];

/// A size (`…Bytes`) reads as the UI's `formatFileSize` writes it (the size of a model chat needs).
#[test]
fn english_words_a_size_as_the_ui_does() {
    let size = |b: f64| english(&[TEST], &CodedError::new("t.model").with("sizeBytes", b));
    assert_eq!(size(190_085_487.0), "a 200 MB model");
    assert_eq!(size(57_200_000.0), "a 58 MB model");
    assert_eq!(size(2_740_937_888.0), "a 2.7 GB model");
    assert_eq!(size(10.0), "a 1 MB model");
}

/// A language a code names (a Whisper code) reads as its English name in Rust's English (MCP replies, logs), as the
/// as the UI names it in the person's language; a value that isn't a code it knows stays as it is.
#[test]
fn english_names_a_language_by_its_name() {
    let again = |l: &str| english(&[TEST], &CodedError::new("t.again").with("language", l));
    assert_eq!(again("de"), "Transcribe it again in German");
    assert_eq!(again("yue"), "Transcribe it again in Cantonese");
    assert_eq!(again("haw"), "Transcribe it again in Hawaiian");
    assert_eq!(again("xx"), "Transcribe it again in xx");
}

#[test]
fn a_code_reaches_the_ui_as_code_and_params_and_text_as_a_string() {
    assert_eq!(
        serde_json::to_value(Said::code("engine.play.rateUnavailable")).unwrap(),
        json!({"code": "engine.play.rateUnavailable", "params": {}})
    );
    assert_eq!(
        serde_json::to_value(Said::from("Budget basics")).unwrap(),
        json!("Budget basics")
    );
    let nested =
        CodedError::new("t.part").with("item", CodedError::new("t.text").with("text", "Step 1"));
    let back: CodedError = serde_json::from_value(serde_json::to_value(&nested).unwrap()).unwrap();
    assert_eq!(back, nested, "a nested code survives the trip");
}

#[test]
fn english_words_times_contributions_numbers_and_nested_codes() {
    let tables = [TEST];
    let cut = CodedError::new("t.cut")
        .with("startUs", 65_000_000i64)
        .with("endUs", 72_000_000i64);
    assert_eq!(english(&tables, &cut), "Cut 1:05–1:12");
    let add = CodedError::new("t.add").with("effect", klipolo_model::contrib::builtin::EFFECT_BLUR);
    assert_eq!(english(&tables, &add), "Add Blur");
    let part =
        CodedError::new("t.part").with("item", CodedError::new("t.text").with("text", "Step 1"));
    assert_eq!(english(&tables, &part), "Part of Text “Step 1”");
    assert_eq!(
        english(&tables, &CodedError::new("t.count").with("count", 3usize)),
        "3 clips"
    );
    assert_eq!(
        english(&tables, &CodedError::new("nope")),
        "nope",
        "an unknown code comes back as is"
    );
    // A moment (`…AtMs`) in UTC to the minute.
    let since = |ms: f64| english(&tables, &CodedError::new("t.since").with("sinceAtMs", ms));
    assert_eq!(since(0.0), "since 1970-01-01 00:00 UTC");
    assert_eq!(since(1_792_050_060_000.0), "since 2026-10-15 07:41 UTC");
    assert_eq!(since(951_782_400_000.0), "since 2000-02-29 00:00 UTC");
}

#[test]
fn model_errors_carry_codes_and_read_in_english_for_logs() {
    let p = fixture();
    let e = apply_ops(&p, &[Op::RemoveClip { id: "nope".into() }]).unwrap_err();
    assert_eq!(e.coded().code, "model.notFound.clip");
    assert_eq!(e.to_string(), "That clip no longer exists.");
    let e = apply_ops(&p, &[Op::Split { at_us: 0 }]).unwrap_err();
    assert!(e.coded().code.starts_with("model."), "{e:?}");
}

#[test]
fn every_model_message_is_unique_and_names_its_params_in_camel_case() {
    let mut seen = std::collections::BTreeSet::new();
    for (code, message) in MESSAGES {
        assert!(seen.insert(*code), "{code} twice");
        assert!(
            code.starts_with("model.") || code.starts_with("name.") || code.starts_with("project."),
            "{code}"
        );
        for name in message
            .split('{')
            .skip(1)
            .map(|s| s.split('}').next().unwrap())
        {
            assert!(
                name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                    && name.chars().all(|c| c.is_ascii_alphanumeric()),
                "{code}: {{{name}}}"
            );
        }
    }
}

#[test]
fn undo_labels_are_message_ids_and_keep_their_params() {
    let mut doc = Document::new(fixture()).unwrap();
    let params = Some(BTreeMap::from([(
        "name".to_string(),
        ErrorParam::from("New"),
    )]));
    let tx = Transaction {
        label: "edit.rename".into(),
        params: params.clone(),
        ops: vec![Op::Rename { name: "New".into() }],
        origin: None,
    };
    doc.apply(&tx).unwrap();
    let h = doc.history();
    assert_eq!(
        (h.undo_label.as_deref(), &h.undo_params),
        (Some("edit.rename"), &params)
    );
    doc.undo().unwrap();
    assert_eq!(
        (
            doc.history().redo_label.as_deref(),
            &doc.history().redo_params
        ),
        (Some("edit.rename"), &params)
    );
    assert_eq!(
        serde_json::to_value(Transaction { params: None, ..tx })
            .unwrap()
            .get("params"),
        None,
        "no params, no key"
    );
}

/// `pnpm bindings` runs this (its name matches `export_bindings`): the model's codes, for the UI's catalogue.
#[test]
fn export_bindings_model_codes() {
    let list: Vec<serde_json::Value> = MESSAGES
        .iter()
        .map(|(code, message)| {
            let params: Vec<&str> = message
                .split('{')
                .skip(1)
                .filter_map(|s| s.split('}').next())
                .collect();
            json!({ "code": code, "message": message, "params": params })
        })
        .collect();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src/bindings");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("modelCodes.json"),
        serde_json::to_string_pretty(&list).unwrap() + "\n",
    )
    .unwrap();
}
