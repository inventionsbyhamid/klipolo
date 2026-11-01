//! Analysis of the open project's clips: thin commands over `klipolo_app::analysis`. The page names clips by asset id;
//! they are looked up in the open project. Progress, a clip done, failed or stopped go out as events
//! (`analysis://progress`, `analysis://asset_done`, `analysis://failed`, `analysis://stopped`), and each new `Speech`
//! goes to the engine's feed for the open revision, so captions update live. No command or event carries a key or a
//! token.

use std::path::Path;
use std::sync::Arc;

use klipolo_app::analysis::{
    AnalysisEvent, AnalysisState, Asked, AssetAnalysisState, EngineMedia, Estimate,
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
/// refused. Only the files a main-track clip plays are analysed (`analysed_assets`); others asked for are left out. A
/// recording's files are priced for their words only. Nothing with no project open.
#[tauri::command]
pub(crate) async fn analysis_estimate(
    analysis: State<'_, AnalysisHandle>,
    projects: State<'_, ProjectSessionState>,
    asset_ids: Vec<String>,
) -> Result<Estimate, String> {
    // The opening first: a switch while the assets are read is seen.
    let opening = analysis.opening();
    let Some((project, assets)) = projects.analysed_assets(&asset_ids) else {
        return Ok(Estimate::default());
    };
    Ok(analysis.estimate_at(opening, &project, &assets).await)
}

/// Analyses these clips (the files a main-track clip plays among them, `analysed_assets`; a recording's for their words
/// only): cached ones load at once; the others start, or the error says why they can't (no key or code, the device not
/// registered, more than the credit left). `by_page`: the page asked on its own (loading cached clips), not the
/// person, so rough words are timed again at most once per clip per app run. Nothing with no project open.
#[tauri::command]
pub(crate) async fn analysis_start(
    analysis: State<'_, AnalysisHandle>,
    projects: State<'_, ProjectSessionState>,
    asset_ids: Vec<String>,
    by_page: Option<bool>,
) -> Result<(), String> {
    // The opening first: a switch while the assets are read is seen.
    let opening = analysis.opening();
    let Some((project, assets)) = projects.analysed_assets(&asset_ids) else {
        return Ok(());
    };
    let asked = if by_page == Some(true) {
        Asked::Page
    } else {
        Asked::Person
    };
    match analysis
        .start_at(opening, &project, &assets, asked)
        .await
        .refused
    {
        Some(reason) => Err(reason),
        None => Ok(()),
    }
}

/// Stops after the requests in flight; their clips go back to not analysed.
#[tauri::command]
pub(crate) fn analysis_cancel(analysis: State<'_, AnalysisHandle>) {
    analysis.cancel();
}

/// These clips left the project: they hear no more about their analysis and lose their captions' speech. Another clip
/// on the same file keeps it; nothing is cancelled.
#[tauri::command]
pub(crate) fn analysis_forget(analysis: State<'_, AnalysisHandle>, asset_ids: Vec<String>) {
    analysis.forget(&asset_ids);
}

/// Every clip analysis knows of, and where it is (a page that loads again reads it).
#[tauri::command]
pub(crate) fn analysis_states(analysis: State<'_, AnalysisHandle>) -> Vec<AssetAnalysisState> {
    analysis.states()
}
