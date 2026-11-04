//! Settings: how Klipolo pays for Meta's models. Thin commands over `klipolo_app::credentials`. Every change is told to
//! the page as an `ai-status` event, so the chat panel, the analysis banner and the top bar follow it. Every status
//! carries what AI cost this session (`SessionSpend`: the analysis, chat and its pictures). Errors are one plain
//! sentence; no command logs or returns a key, a token or a code it was given.

use std::path::Path;
use std::sync::Arc;

use klipolo_app::credentials::{
    AiStatus, Credentials, Endpoints, SecretStore, built_in_trial_code, device_id,
};
use tauri::{AppHandle, Emitter, Manager, State};

use super::analysis::AnalysisHandle;
use super::chat::ChatHandle;
use super::speaker::SpeakerHandle;

/// The event every AI status change goes out on.
pub(crate) const AI_STATUS_EVENT: &str = "ai-status";

/// The app's credentials and its one shared Meta adapter.
pub(crate) type CredentialsState = Arc<Credentials>;

/// The credentials saved on this Mac, with this device's id from `support` (app support `Klipolo/`) and the free credit
/// built into this copy, if any. Reads the Keychain; makes no network call.
pub(crate) fn load(support: Option<&Path>) -> CredentialsState {
    let device = support
        .and_then(|dir| {
            device_id(dir)
                .inspect_err(|error| tracing::warn!(%error, "device id"))
                .ok()
        })
        // Without app support the id lasts this run only; the relay sees a new device next time.
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    Arc::new(
        Credentials::load(store(), Endpoints::for_app(), device)
            .with_trial_code(built_in_trial_code()),
    )
}

#[cfg(target_os = "macos")]
fn store() -> Arc<dyn SecretStore> {
    use klipolo_app::credentials::{KeychainStore, SERVICE};
    Arc::new(KeychainStore::new(SERVICE))
}

/// No Keychain here: secrets last as long as the app runs.
#[cfg(not(target_os = "macos"))]
fn store() -> Arc<dyn SecretStore> {
    tracing::warn!(
        "no Keychain on this system: a Meta key or access code is kept in memory only, until the app quits"
    );
    Arc::new(klipolo_app::credentials::MemoryStore::default())
}

/// `status` with what AI cost this session.
fn with_session(app: &AppHandle, mut status: AiStatus) -> AiStatus {
    // Following the speaker is footage analysis too: its SAM calls count with the analysis's.
    let analysis_usd = app
        .try_state::<AnalysisHandle>()
        .map_or(0.0, |a| a.spent_usd())
        + app
            .try_state::<SpeakerHandle>()
            .map_or(0.0, |s| s.spent_usd());
    let chat = app
        .try_state::<ChatHandle>()
        .map(|c| c.spend())
        .unwrap_or_default();
    status.session = Some(chat.with_analysis(analysis_usd));
    status
}

/// Tells the page the AI status changed.
pub(crate) fn announce(app: &AppHandle, status: &AiStatus) {
    if let Err(error) = app.emit(AI_STATUS_EVENT, status) {
        tracing::warn!(%error, "ai status event");
    }
}

/// What Settings shows: the mode, and the access code's credit and spend (asked from the relay).
#[tauri::command]
pub(crate) async fn settings_get(
    app: AppHandle,
    credentials: State<'_, CredentialsState>,
) -> Result<AiStatus, String> {
    Ok(with_session(&app, credentials.status().await))
}

/// Checks the person's Meta API key with Meta and saves it in the Keychain only when it works.
#[tauri::command]
pub(crate) async fn settings_set_key(
    app: AppHandle,
    credentials: State<'_, CredentialsState>,
    key: String,
) -> Result<AiStatus, String> {
    let status = with_session(
        &app,
        credentials.set_key(&key).await.map_err(|e| e.message)?,
    );
    announce(&app, &status);
    Ok(status)
}

/// Registers this Mac with the access code and saves the relay's token.
#[tauri::command]
pub(crate) async fn settings_set_code(
    app: AppHandle,
    credentials: State<'_, CredentialsState>,
    code: String,
) -> Result<AiStatus, String> {
    let status = with_session(
        &app,
        credentials.set_code(&code).await.map_err(|e| e.message)?,
    );
    announce(&app, &status);
    Ok(status)
}

/// Removes the key and the code: the free credit pays again when this copy has one, else AI features ask for a key or
/// code; editing and export keep working.
#[tauri::command]
pub(crate) async fn settings_clear(
    app: AppHandle,
    credentials: State<'_, CredentialsState>,
) -> Result<AiStatus, String> {
    let status = with_session(&app, credentials.clear().await.map_err(|e| e.message)?);
    announce(&app, &status);
    Ok(status)
}
