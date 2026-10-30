//! Analysis of the open project's clips: thin commands over `klipolo_app::analysis`. The page names clips by asset id;
//! they are looked up in the open project. Progress, a clip done, failed or stopped go out as events
//! (`analysis://progress`, `analysis://asset_done`, `analysis://failed`, `analysis://stopped`), and each new `Speech`
//! goes to the engine's feed for the open revision, so captions update live. No command or event carries a key or a
//! token.

use std::path::Path;
use std::sync::Arc;

use klipolo_app::analysis::{
    AnalysisEvent, AnalysisState, AssetAnalysisState, EngineMedia, Estimate,
};
use klipolo_engine::audio::pcm::PcmStore;
use tauri::{AppHandle, Emitter, Manager, State};

use super::settings::CredentialsState;
use crate::project::ProjectSessionState;

/// The app's analysis.
pub(crate) type AnalysisHandle = Arc<AnalysisState>;

/// The app's analysis: its cache in `support/analysis` (app support `Klipolo/`), its working files in the app's cache
/// folder, its levels from `sounds` (the sound cache the preview shares).
pub(crate) fn load(
    app: &AppHandle,
    support: Option<&Path>,
    credentials: CredentialsState,
    sounds: Option<Arc<PcmStore>>,
) -> AnalysisHandle {
    let temp = std::env::temp_dir().join("klipolo");
    let cache_root = support.map_or_else(|| temp.join("analysis"), |dir| dir.join("analysis"));
    let work_root = app.path().app_cache_dir().map_or_else(
        |_| temp.join("analysis-work"),
        |dir| dir.join("analysis-work"),
    );
    let events = {
        let app = app.clone();
        Arc::new(move |event: AnalysisEvent| {
            if let Err(error) = app.emit(event.name(), &event) {
                tracing::warn!(%error, "analysis event");
            }
        })
    };
    let speech = {
        let app = app.clone();
        Arc::new(move |speech| app.state::<ProjectSessionState>().set_speech(speech))
    };
    AnalysisState::new(
        credentials,
        Arc::new(EngineMedia::new(sounds)),
        cache_root,
        work_root,
        events,
        speech,
    )
}

/// What analysing these clips costs, which are cached, and, with an access code, the credit left and why Analyze is
/// refused.
#[tauri::command]
pub(crate) async fn analysis_estimate(
    analysis: State<'_, AnalysisHandle>,
    projects: State<'_, ProjectSessionState>,
    asset_ids: Vec<String>,
) -> Result<Estimate, String> {
    let assets = projects.assets(&asset_ids);
    Ok(analysis.estimate(&assets).await)
}

/// Analyses these clips: cached ones load at once; the others start, or the error says why they can't (no key or code,
/// the device not registered, more than the credit left).
#[tauri::command]
pub(crate) async fn analysis_start(
    analysis: State<'_, AnalysisHandle>,
    projects: State<'_, ProjectSessionState>,
    asset_ids: Vec<String>,
) -> Result<(), String> {
    let assets = projects.assets(&asset_ids);
    match analysis.start(&assets).await.refused {
        Some(reason) => Err(reason),
        None => Ok(()),
    }
}

/// Stops after the requests in flight; their clips go back to not analysed.
#[tauri::command]
pub(crate) fn analysis_cancel(analysis: State<'_, AnalysisHandle>) {
    analysis.cancel();
}

/// Every clip analysis knows of, and where it is (a page that loads again reads it).
#[tauri::command]
pub(crate) fn analysis_states(analysis: State<'_, AnalysisHandle>) -> Vec<AssetAnalysisState> {
    analysis.states()
}
