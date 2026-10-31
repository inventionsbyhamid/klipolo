#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod engine;
mod english;
mod log;
mod media;
mod media_cache;
mod media_jobs;
mod pack;
mod project;
mod state;

use std::sync::Arc;

use klipolo_engine::render::fonts::{self, FontDirs};
use klipolo_engine::render::mode::RenderMode;
use klipolo_engine::render::warm::Warmup;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};

use crate::project::{ProjectSessionState, SaveStatus};
use crate::state::EngineState;

fn main() {
    // Export and editing copies run out of process in this same binary: no second executable, always the same
    // version. Before Tauri starts and before any window exists; their log is stderr, which the app forwards into its
    // own.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--export-worker") {
        log::install_worker();
        std::process::exit(klipolo_engine::export::worker::main(&args[2..]));
    }
    if args.get(1).map(String::as_str) == Some("--proxy-worker") {
        log::install_worker();
        std::process::exit(klipolo_engine::media::proxy::worker::main(&args[2..]));
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
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            let cache = app.path().app_cache_dir().ok();
            app.manage(EngineState::new(Some(warm), cache));
            let projects = ProjectSessionState::default();
            // Klipolo's pack where this install has it; a project's pack files point here as it opens.
            projects.set_pack(
                pack::pack_dir(app.path().resource_dir().ok())
                    .map(|dir| pack::Pack::read(&dir))
                    .unwrap_or_default(),
            );
            let handle = app.handle().clone();
            projects.set_save_report(Arc::new(move |status: SaveStatus| {
                let _ = handle.emit("project-save", status);
            }));
            // Klipolo opens on the start screen, with no project open; the projects opened from it are remembered in
            // the app's data folder, for its list.
            if let Ok(data) = app.path().app_data_dir() {
                projects.at_launch(&data);
            }
            // The Captions panel's pages, one build kept; new speech on the feed is told to the page.
            app.manage(klipolo_app::captions::CaptionsCache::default());
            commands::captions::follow_speech(app.handle(), &projects);
            app.manage(projects);
            // How Klipolo pays for Meta's models (the Keychain), and this Mac's id with the relay (app support).
            let support = app.path().data_dir().ok().map(|dir| dir.join("Klipolo"));
            let credentials = commands::settings::load(support.as_deref());
            app.manage(credentials.clone());
            // The clips' analysis, cached in app support, its levels from the sound cache the preview shares.
            let sounds = app.state::<EngineState>().sounds.clone();
            let analysis = commands::analysis::load(
                app.handle(),
                support.as_deref(),
                credentials.clone(),
                sounds,
            );
            // Chat over the open project, what analysis found and the same credentials.
            let chat = commands::chat::load(app.handle(), credentials, analysis.clone());
            // Another project opening (or this one closing) stops analysis and starts chat over, once the switch is
            // sure.
            let (stopping, forgetting) = (analysis.clone(), chat.clone());
            app.state::<ProjectSessionState>()
                .on_switch(Arc::new(move || {
                    stopping.project_changed();
                    forgetting.project_changed();
                }));
            app.manage(analysis);
            app.manage(chat);
            app.manage(commands::export::state_at_launch());
            // Editing copies of the open project's slow files, beside the thumbnails in the media cache. The preview
            // reads them once made; they wait while an export holds the slot.
            let proxies = app
                .state::<EngineState>()
                .caches()
                .map_or_else(
                    || std::env::temp_dir().join("Klipolo"),
                    |caches| caches.media,
                )
                .join("proxies");
            let handle = app.handle().clone();
            let copies = media_jobs::MediaJobs::new(
                proxies,
                media_jobs::launcher(),
                Arc::new(move |list| {
                    let _ = handle.emit("media-jobs", list);
                }),
            );
            copies.start();
            copies.watch(app.state::<ProjectSessionState>().subscribe());
            let exporting = copies.clone();
            app.state::<klipolo_app::export::ExportState>()
                .on_busy(Arc::new(move |busy| exporting.set_exporting(busy)));
            app.manage(copies);
            commands::export::guard_menu_quit(app)?;
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
            engine::engine_set_editing,
            project::project_create,
            project::project_open,
            project::project_close,
            project::project_apply,
            project::project_undo,
            project::project_redo,
            project::project_snapshot,
            project::project_reveal,
            project::projects_recent,
            media::media_import,
            commands::settings::settings_get,
            commands::settings::settings_set_key,
            commands::settings::settings_set_code,
            commands::settings::settings_clear,
            commands::analysis::analysis_estimate,
            commands::analysis::analysis_start,
            commands::analysis::analysis_cancel,
            commands::analysis::analysis_states,
            commands::analysis::analysis_forget,
            commands::chat::chat_send,
            commands::chat::chat_apply_proposal,
            commands::chat::chat_discard_proposal,
            commands::chat::chat_undo_last,
            commands::chat::chat_reset,
            commands::chat::chat_draw_picture,
            commands::chat::chat_use_picture,
            media_cache::media_peaks_info,
            media_cache::media_peaks,
            media_cache::media_thumbs,
            commands::media::media_add_music,
            commands::media::media_add_sound,
            commands::media::media_add_image,
            commands::media::media_place,
            commands::media::media_add_text,
            commands::media::media_update_item,
            commands::media::media_picture,
            commands::media::media_speech_ranges,
            commands::media::library_pack,
            commands::fonts::fonts_bundled,
            commands::fonts::fonts_face,
            commands::captions::captions_pages,
            commands::captions::captions_find,
            commands::captions::dev_fixture_speech,
            commands::export::export_plan,
            commands::export::export_start,
            commands::export::export_cancel,
            commands::export::export_reveal,
            commands::export::export_open,
            commands::export::export_files,
            commands::export::export_pick_folder,
            media_jobs::commands::media_jobs_list,
            media_jobs::commands::media_jobs_pause,
            media_jobs::commands::media_jobs_resume,
            media_jobs::commands::media_job_stop,
            media_jobs::commands::media_job_retry,
            media_jobs::commands::media_jobs_set_auto,
        ])
        .build(tauri::generate_context!())
        .expect("Klipolo could not start");
    app.run(|app, event| {
        // Quitting while an export runs asks first.
        commands::export::guard_quit(app, &event);
        match event {
            // Every edit is on the disk before the app goes.
            RunEvent::Exit => commands::export::on_exit(app),
            // Back in front: a source exported again elsewhere meanwhile must not keep showing its old editing copy.
            RunEvent::WindowEvent {
                event: WindowEvent::Focused(true),
                ..
            } => {
                if let Some(copies) = app.try_state::<media_jobs::MediaJobs>() {
                    copies.recheck();
                }
            }
            _ => {}
        }
    });
}
