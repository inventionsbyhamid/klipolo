//! The recorder's Tauri commands: thin adapters over `RecorderState`. None of them reaches chat or MCP, and capture
//! starts only on the person's own Record press (`recorder_send` with `Start`). Device listing, files, the probe and
//! the worker's launch run on a blocking thread, never the page's.
//!
//! Event: `recorder-event` (`RecorderEvent`: the worker's `CaptureEvent`s and the recorder's own `Failed`, notes and
//! `State Idle`, each with its take's id).

use klipolo_app::export::ExportState;
use klipolo_capture::preflight::RecorderSetup;
use klipolo_capture::protocol::Command;
use klipolo_model::coded::Said;
use tauri::{AppHandle, Manager};

use super::permissions::open_url;
use super::state::RecorderState;
use super::types::{ArmOptions, ArmedTake, PrivacyPage, StoppedTake};
use super::words;
use crate::project::ProjectSessionState;

/// Runs `work` on the blocking pool with the recorder's state.
async fn blocking<T: Send + 'static>(
    app: AppHandle,
    work: impl FnOnce(&AppHandle, &RecorderState) -> Result<T, Said> + Send + 'static,
) -> Result<T, Said> {
    tauri::async_runtime::spawn_blocking(move || work(&app, &app.state::<RecorderState>()))
        .await
        .map_err(|e| Said::Coded(words::os_error(&e.to_string())))?
}

/// The Record dialog's data: devices, permissions, each display's qualities and space, and the notes. No capture and
/// no prompt. In a test run, `capture.error.testRun`.
#[tauri::command]
pub async fn recorder_setup(app: AppHandle) -> Result<RecorderSetup, Said> {
    blocking(app, |_, state| state.setup().map_err(Said::Coded)).await
}

/// Launches the take's worker, armed: nothing is captured until `recorder_send(Start)`. `capture.busy` while a take
/// runs.
#[tauri::command]
pub async fn recorder_arm(app: AppHandle, options: ArmOptions) -> Result<ArmedTake, Said> {
    blocking(app, move |_, state| {
        state.arm(&options).map_err(Said::Coded)
    })
    .await
}

/// One command to take `take`'s worker (`ArmedTake.id`): `Start` is the person's Record press. A take that isn't the
/// current one is refused with `capture.error.staleTake`.
#[tauri::command]
pub async fn recorder_send(app: AppHandle, take: String, command: Command) -> Result<(), Said> {
    blocking(app, move |_, state| {
        state.send(&take, command).map_err(Said::Coded)
    })
    .await
}

/// "Stop the running recording": Stop for whatever take runs, whichever id the page lost. Its id and whether it had
/// started, or None when there is none.
#[tauri::command]
pub async fn recorder_stop_running(app: AppHandle) -> Result<Option<StoppedTake>, Said> {
    blocking(app, |_, state| state.stop_running().map_err(Said::Coded)).await
}

/// "Allow screen recording": asks macOS, then opens Screen Recording's settings page.
#[tauri::command]
pub async fn recorder_allow_screen(app: AppHandle) -> Result<(), Said> {
    blocking(app, |_, state| {
        state.allow_screen(&open_url).map_err(Said::Coded)
    })
    .await
}

/// "Reopen Klipolo": the project is saved, then Klipolo starts again. `capture.busy` during a take, and
/// `capture.permission.reopenExporting` while an export runs, before or after the save (Tauri's restart can't be held
/// by the export's quit question); a save that fails keeps Klipolo open and says why. A second press while it is under
/// way does nothing more.
#[tauri::command]
pub async fn recorder_reopen(app: AppHandle) -> Result<(), Said> {
    blocking(app, |app, state| {
        let flush =
            || (app.state::<ProjectSessionState>().flush()).map_err(|e| Said::Text(e.to_string()));
        let exporting =
            || (app.try_state::<ExportState>()).is_some_and(|export| export.is_running());
        state.reopen(&exporting, flush, || app.request_restart())
    })
    .await
}

/// "Open privacy settings": Screen Recording's, the camera's or the microphone's page.
#[tauri::command]
pub async fn recorder_open_privacy(app: AppHandle, page: PrivacyPage) -> Result<(), Said> {
    blocking(app, move |_, state| {
        state.open_privacy(page, &open_url).map_err(Said::Coded)
    })
    .await
}
