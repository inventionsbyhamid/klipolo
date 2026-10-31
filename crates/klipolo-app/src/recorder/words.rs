//! What the recorder says. Its refusals and notes are `capture.*` codes with English in `klipolo_capture::english`,
//! which the page words (`bindings/captureCodes.json`), never English of the app's own. The worker's menu-bar item and
//! HUD are given no words (`SessionConfig.words` is empty), so they read in English from the same table.

use std::collections::BTreeMap;

use klipolo_capture::backend::IndicatorPlace;
use klipolo_model::coded::CodedError;

/// The words the worker's menu and HUD are given: none, so each reads its English.
pub(super) fn worker_words() -> BTreeMap<String, String> {
    BTreeMap::new()
}

/// "Recording is off in this test run." (`KLIPOLO_TEST`): the recorder made no backend and launches no worker.
pub(super) fn test_run() -> CodedError {
    CodedError::new("capture.error.testRun")
}

/// "A recording is already running. Stop it from {place}."
pub(super) fn busy(place: IndicatorPlace) -> CodedError {
    CodedError::new("capture.busy").with("place", place.coded())
}

/// "That recording has already ended."
pub(super) fn stale_take() -> CodedError {
    CodedError::new("capture.error.staleTake")
}

/// "This computer can't record that.": a command the page may not send, or no place for the recorder's files.
pub(super) fn refused() -> CodedError {
    CodedError::new("capture.error.unsupported")
}

/// "That device isn't connected.": an id the OS couldn't have given.
pub(super) fn no_device() -> CodedError {
    CodedError::new("capture.error.noDevice")
}

/// "Recording failed.", with the cause for the log only.
pub(super) fn os_error(detail: &str) -> CodedError {
    tracing::warn!(detail, "recorder: a take couldn't start");
    CodedError::new("capture.error.os").with("detail", detail)
}
