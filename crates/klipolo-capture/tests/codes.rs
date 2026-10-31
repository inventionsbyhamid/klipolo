use std::collections::BTreeSet;
use std::path::PathBuf;

/// `apps/desktop/src/bindings/captureCodes.json`: this crate's codes, their English and their values, for the UI's
/// catalogue (written exactly as klipolo-model's `export_bindings_model_codes` writes `modelCodes.json`).
#[test]
fn export_bindings_capture_codes() {
    let list: Vec<serde_json::Value> = klipolo_capture::english::ENGLISH
        .iter()
        .map(|(code, message)| {
            let params: Vec<&str> = message
                .split('{')
                .skip(1)
                .filter_map(|s| s.split('}').next())
                .collect();
            serde_json::json!({ "code": code, "message": message, "params": params })
        })
        .collect();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src/bindings");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("captureCodes.json"),
        serde_json::to_string_pretty(&list).unwrap() + "\n",
    )
    .unwrap();
}

#[test]
fn every_code_has_english_and_no_ui_namespace() {
    let mut seen = BTreeSet::new();
    for (code, message) in klipolo_capture::english::ENGLISH {
        // The UI's own messages are `recorder.*`, `source.*` and `marker.*`; `capture` is Rust's.
        assert!(code.starts_with("capture."), "{code} is outside capture.*");
        assert!(!message.trim().is_empty(), "{code} has no English");
        assert!(seen.insert(*code), "{code} is listed twice");
    }
}

#[test]
fn the_space_left_reads_in_hours_and_minutes() {
    use klipolo_capture::english::{english, hours_minutes};
    let space = |minutes| {
        english(
            &klipolo_model::coded::CodedError::new("capture.menu.space")
                .with("left", hours_minutes(minutes)),
        )
    };
    assert_eq!(space(70), "Space left: about 1 h 10 min");
    assert_eq!(space(120), "Space left: about 2 h");
    assert_eq!(space(45), "Space left: about 45 min");
}

#[test]
fn a_permission_still_asked_reads_as_waiting_not_denied() {
    // macOS's camera prompt not answered in time is not a refusal.
    use klipolo_capture::english::english;
    use klipolo_capture::pipe::PipeError;
    let pending = english(&PipeError::PermissionPending.coded());
    assert!(
        pending.contains("still asking whether Klipolo may"),
        "{pending}"
    );
    assert!(
        !pending.contains("allow"),
        "it doesn't presume the answer: {pending}"
    );
    assert_ne!(pending, english(&PipeError::PermissionDenied.coded()));
}
