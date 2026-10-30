#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod log;
mod project;
mod state;

use std::sync::Arc;

use klipolo_engine::render::fonts::{self, FontDirs};
use klipolo_engine::render::mode::RenderMode;
use klipolo_engine::render::warm::Warmup;
use tauri::{Emitter, Manager, RunEvent};

use crate::project::{ProjectSessionState, SaveStatus};
use crate::state::EngineState;

fn main() {
    // Export runs out of process in this same binary: no second executable, always the same version. Before Tauri
    // starts and before any window exists; its log is stderr, which the app forwards into its own.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--export-worker") {
        log::install_worker();
        std::process::exit(klipolo_engine::export::worker::main(&args[2..]));
    }
    log::install();
    // The engine's fonts, once: the bundled folder in the app's resources (or the repository's in a dev build).
    if let Err(e) = fonts::configure(FontDirs {
        bundled: klipolo_fonts::bundled::bundled_dir(),
    }) {
        tracing::warn!(%e, "fonts");
    }
    // The preview's shaders compile on a thread of their own while the window loads; the engine's first start takes
    // the backend.
    let warm = Warmup::start(RenderMode::Interactive);
    let app = tauri::Builder::default()
        .setup(move |app| {
            let cache = app.path().app_cache_dir().ok();
            app.manage(EngineState::new(Some(warm), cache));
            let projects = ProjectSessionState::default();
            let handle = app.handle().clone();
            projects.set_save_report(Arc::new(move |status: SaveStatus| {
                let _ = handle.emit("project-save", status);
            }));
            // The project open when the app last quit opens again; the page reads it with `project_snapshot`.
            if let Ok(data) = app.path().app_data_dir() {
                projects.remember_last_in(data.join("last-project"));
                projects.open_last();
            }
            app.manage(projects);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine::engine_attach,
            engine::engine_view,
            engine::engine_set_view,
            engine::engine_set_visible,
            engine::engine_seek,
            engine::engine_preview,
            engine::engine_preview_clear,
            engine::engine_play,
            engine::engine_pause,
            project::project_create,
            project::project_open,
            project::project_close,
            project::project_apply,
            project::project_undo,
            project::project_redo,
            project::project_snapshot,
            project::project_summary,
        ])
        .build(tauri::generate_context!())
        .expect("Klipolo could not start");
    app.run(|app, event| {
        // Every edit is on the disk before the app goes.
        if let RunEvent::Exit = event
            && let Err(error) = app.state::<ProjectSessionState>().flush()
        {
            tracing::warn!(
                detail = error.detail(),
                "the last edits couldn't be saved at quit"
            );
        }
    });
}
