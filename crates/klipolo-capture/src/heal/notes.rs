//! The heal policy's notes: a lost source, a mic that keeps dropping, and a pipe's own one-time notes.

use klipolo_model::coded::ErrorParam;

use super::{Heal, HealAction};
use crate::english::{ENGLISH, code};
use crate::health::{HealthNote, source_param};
use crate::shell::Level;

/// `capture.health.lost` ("{source} disconnected."), with the reason and the encoder's status for the log.
pub(super) fn lost(source: &str, reason: &str, status: Option<i32>) -> HealthNote {
    let mut params = vec![
        ("source", source_param(source)),
        ("reason", ErrorParam::from(reason)),
    ];
    if let Some(status) = status {
        params.push(("status", ErrorParam::from(i64::from(status))));
    }
    HealthNote {
        code: code!("capture.health.lost"),
        source: Some(source.into()),
        params,
        level: Level::Warning,
    }
}

/// `capture.note.micDropping`: a mild note (no warning, no flash) that the mic keeps dropping and is recorded again each
/// time it is back.
pub(super) fn dropping(source: &str) -> HealthNote {
    HealthNote {
        code: code!("capture.note.micDropping"),
        source: Some(source.into()),
        params: vec![("source", source_param(source))],
        level: Level::Ok,
    }
}

/// A pipe's one-time warning: a code with English, once per take and source (an unknown code is logged and ignored).
pub(super) fn pipe_note(h: &mut Heal, source: &str, wanted: &str) -> HealAction {
    let Some(&(code, english)) = ENGLISH.iter().find(|(c, _)| *c == wanted) else {
        tracing::warn!(code = wanted, source, "a pipe's note has no English");
        return HealAction::Ignore;
    };
    if h.noted.contains(&code) {
        return HealAction::Ignore;
    }
    h.noted.push(code);
    let params = if english.contains("{source}") {
        vec![("source", source_param(source))]
    } else {
        vec![]
    };
    // The `capture.note.*` family is mild (a flapping mic waiting to settle): no warning, no flash.
    let level = if code.starts_with("capture.note.") {
        Level::Ok
    } else {
        Level::Warning
    };
    HealAction::Note(HealthNote {
        code,
        source: Some(source.into()),
        params,
        level,
    })
}
