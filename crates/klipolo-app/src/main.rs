#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod log;
mod state;

use klipolo_engine::render::fonts::{self, FontDirs};
use klipolo_engine::render::mode::RenderMode;
use klipolo_engine::render::warm::Warmup;
use tauri::Manager;

use crate::state::{EngineState, ProjectState};

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
    let projects = ProjectState::default();
    if let Some(path) = state::launch_video() {
        match state::single_video_project(&path) {
            Ok(project) => projects.open(project),
            Err(error) => tracing::warn!(%error, "the launch video couldn't be opened"),
        }
    }
    tauri::Builder::default()
        .setup(move |app| {
            let cache = app.path().app_cache_dir().ok();
            app.manage(EngineState::new(Some(warm), cache));
            Ok(())
        })
        .manage(projects)
        .invoke_handler(tauri::generate_handler![
            engine::engine_attach,
            engine::engine_view,
            engine::engine_set_view,
            engine::engine_seek,
            engine::engine_play,
            engine::engine_pause,
            state::project_summary,
        ])
        .run(tauri::generate_context!())
        .expect("Klipolo could not start");
}
