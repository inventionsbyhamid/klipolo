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

/// "This computer can't record that.": a command the page may not send.
pub(super) fn refused() -> CodedError {
    CodedError::new("capture.error.unsupported")
}

/// "That device isn't connected.": an id the OS couldn't have given.
pub(super) fn no_device() -> CodedError {
    CodedError::new("capture.error.noDevice")
}

/// "Recording failed.", with the cause for the log only.
pub(super) fn os_error(detail: &str) -> CodedError {
    tracing::warn!(detail, "recorder: something the recorder needs failed");
    CodedError::new("capture.error.os").with("detail", detail)
}

/// A take's folder or config couldn't be made: a full disk says so ("There isn't enough space to record. Free up at
/// least 2 GB."), anything else "The recording couldn't be set up. Check that your Movies folder can be written to,
/// then press Record again.", with the cause for the log only.
pub(super) fn set_up_failed(error: &std::io::Error) -> CodedError {
    tracing::warn!(%error, "recorder: a take couldn't be set up");
    // ENOSPC on macOS and Linux.
    let full = error.kind() == std::io::ErrorKind::StorageFull || error.raw_os_error() == Some(28);
    if full {
        return CodedError::new("capture.preflight.diskBlock");
    }
    CodedError::new("capture.error.setUp").with("detail", error.to_string())
}

/// The take's worker couldn't be launched: "The recorder didn't start. Reopen Klipolo, then press Record again.", with
/// the cause for the log only.
pub(super) fn worker_didnt_start(error: &std::io::Error) -> CodedError {
    tracing::warn!(%error, "recorder: the take's worker couldn't be launched");
    CodedError::new("capture.error.workerDidntStart").with("detail", error.to_string())
}

/// "Klipolo can't find its settings folder, so it can't record. Reopen Klipolo, then try again."
pub(super) fn no_settings_folder() -> CodedError {
    CodedError::new("capture.error.noSettingsFolder")
}

/// "Klipolo isn't allowed to record that yet.": Screen Recording isn't allowed, so Record would make macOS ask.
pub(super) fn not_allowed() -> CodedError {
    CodedError::new("capture.error.permission")
}

/// "System Settings didn't open. Open it from the Apple menu, then choose Privacy & Security."
pub(super) fn settings_didnt_open(error: &std::io::Error) -> CodedError {
    tracing::warn!(%error, "recorder: a privacy settings page didn't open");
    CodedError::new("capture.permission.settingsDidntOpen").with("detail", error.to_string())
}

/// "An export is running. Reopen Klipolo when it's done."
pub(super) fn reopen_exporting() -> CodedError {
    CodedError::new("capture.permission.reopenExporting")
}
