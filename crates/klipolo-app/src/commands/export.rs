//! The export dialog's commands, thin layers over `klipolo_app::export`: plan the files with what they
//! cost, start an export on the one slot (its progress on a Channel), cancel it, open or show its files, pick the
//! folder they go to; and the quit guard, which asks before a running export is stopped by quitting. Errors are codes
//! the page words (`bridge/call.ts`).

use std::future::Future;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use klipolo_app::export::{
    self, ExportPlan, ExportState, ExportedFile, Releases, StartSteps, Started,
};
use klipolo_engine::api::{ExportEvent, ExportRequest};
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::runner::{Batch, ProcessLauncher, discard_job};
use klipolo_engine::render::fonts;
use klipolo_engine::session::ProjectFeed;
use klipolo_model::coded::Said;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, RunEvent, State, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

use crate::project::ProjectSessionState;

/// How long a start waits for this Mac's fonts to be read before checking them: past it nothing is called missing,
/// and the worker still stops on a frame it would draw with a stand-in.
const FONTS_WAIT: Duration = Duration::from_secs(2);
/// How long Quit waits for a stopped export to let go of its slot before the app goes anyway.
const STOP_WAIT: Duration = Duration::from_secs(10);

/// The open project as the engine sees it (the latest edit in), with what was said and its text's language.
fn open_feed(session: &ProjectSessionState) -> Result<ProjectFeed, Said> {
    session
        .subscribe()
        .borrow()
        .clone()
        .ok_or_else(|| Said::code("exports.noProject"))
}

/// Where the files go: the folder the person picked, else `Movies / Klipolo`.
fn folder(state: &ExportState) -> Result<PathBuf, Said> {
    state.folder().map_or_else(export::default_folder, Ok)
}

/// This Mac has a media engine (asked once).
fn apple_silicon() -> bool {
    static FAST: OnceLock<bool> = OnceLock::new();
    *FAST.get_or_init(|| export::apple_silicon(&klipolo_platform::caps::current()))
}

/// What exporting `req`'s formats of the open project writes and costs. Nothing is written.
#[tauri::command(async)]
pub(crate) fn export_plan(
    state: State<'_, ExportState>,
    session: State<'_, ProjectSessionState>,
    req: ExportRequest,
) -> Result<ExportPlan, Said> {
    let feed = open_feed(&session)?;
    Ok(export::plan(
        &feed.project,
        &req,
        &folder(&state)?,
        apple_silicon(),
    ))
}

/// The app's side of a start: the open project, `prepare`, the job written off the async threads, and a job that
/// won't run deleted.
struct AppSteps<'a> {
    app: &'a AppHandle,
    dir: PathBuf,
}

impl StartSteps for AppSteps<'_> {
    fn open(&self) -> Option<(String, String)> {
        let feed = open_feed(&self.app.state::<ProjectSessionState>()).ok()?;
        Some((feed.project.id.clone(), feed.project.name.clone()))
    }

    fn prepare(&self, req: &ExportRequest) -> impl Future<Output = Result<Batch, Said>> + Send {
        prepare(self.app.clone(), req.clone(), self.dir.clone())
    }

    fn create(&self, batch: Batch) -> impl Future<Output = Result<ExportJob, Said>> + Send {
        export::create_job(batch)
    }

    fn drop_new(&self, job: &ExportJob) {
        if let Err(why) = discard_job(&job.dir) {
            tracing::warn!(%why, "export: a job written for a start that didn't run couldn't be deleted");
        }
    }
}

/// Saves the latest edits (the export reads the project as saved), then freezes the open project into a batch for
/// `dir`: refused when it has nothing to show or uses a font that isn't on this Mac, and when the folder can't be made.
async fn prepare(app: AppHandle, req: ExportRequest, dir: PathBuf) -> Result<Batch, Said> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || handle.state::<ProjectSessionState>().flush())
        .await
        .map_err(|_| Said::code("exports.notStarted"))?
        .map_err(|error| {
            tracing::warn!(detail = error.detail(), "an export's save failed");
            Said::code("exports.unsaved")
        })?;
    let feed = open_feed(&app.state::<ProjectSessionState>())?;
    export::refuse_empty(&feed.project)?;
    fonts_checked(&feed, req.allow_stand_ins).await?;
    std::fs::create_dir_all(&dir).map_err(|e| export::folder_error(&e, &dir))?;
    // Jobs an export cut off by a crash left behind go now (only Klipolo's: `export::tidy_jobs`).
    let tidied = dir.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || export::tidy_jobs(&tidied)).await;
    // Decoded sound comes from (and goes to) the preview's own cache, under its budget: the files the open project
    // uses, and an unfinished job's, are never evicted from it (`start_engine`'s cache tidy).
    let pcm_dir = app
        .state::<EngineState>()
        .caches()
        .map(|caches| caches.audio);
    Ok(export::freeze(
        feed.project,
        &feed.speech,
        feed.text_language,
        &req,
        &dir,
        None,
    ))
}

/// Never a stand-in drawn silently: once this Mac's fonts are read (waiting `FONTS_WAIT` at most), a font the video
/// draws that isn't here refuses the export, naming it.
async fn fonts_checked(feed: &ProjectFeed, allow_stand_ins: bool) -> Result<(), Said> {
    let mut scanned = fonts::subscribe_scanned();
    let read = tokio::time::timeout(FONTS_WAIT, scanned.wait_for(|done| *done))
        .await
        .is_ok_and(|seen| seen.is_ok());
    if !read {
        return Ok(());
    }
    let missing = export::missing_fonts(&fonts::index(), &feed.project, &feed.text_language);
    export::refuse_missing(&missing, allow_stand_ins)
}

/// Runs `job` in the background on the slot it holds, telling the page on `events`, and lets go of the slot when it
/// ends.
fn spawn_run(
    app: &AppHandle,
    launcher: ProcessLauncher,
    job: ExportJob,
    cancel: tokio::sync::watch::Receiver<bool>,
    events: Channel<ExportEvent>,
) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = handle.state::<ExportState>();
        // Released however the run ends, a panic included.
        let _slot = Releases(state.inner());
        let emit = |event: ExportEvent| {
            state.record(&event);
            let _ = events.send(event);
        };
        let outcome = export::run(&launcher, job, cancel, &emit).await;
        tracing::info!(?outcome, "an export ended");
    });
}

/// Starts exporting `req`'s formats of the open project and returns once it has started; progress arrives on
/// `events`. Cancel works from the moment it's called: one while the latest edits are saved ends it with `Cancelled`
/// before anything is written.
#[tauri::command]
pub(crate) async fn export_start(
    app: AppHandle,
    state: State<'_, ExportState>,
    req: ExportRequest,
    events: Channel<ExportEvent>,
) -> Result<(), Said> {
    if req.formats.is_empty() {
        return Err(Said::code("exports.noFormat"));
    }
    // The worker is this app's own binary.
    let launcher = std::env::current_exe()
        .map(ProcessLauncher::export)
        .map_err(|_| Said::code("exports.notStarted"))?;
    let steps = AppSteps {
        app: &app,
        dir: folder(&state)?,
    };
    match export::start_on_slot(&state, &req, &steps).await? {
        Started::Run(job, cancel) => spawn_run(&app, launcher, *job, cancel, events),
        Started::Cancelled => {
            let _ = events.send(ExportEvent::Cancelled);
        }
    }
    Ok(())
}

/// Stops the running export and deletes its job and parts. `job` names it (the run on screen); without it only the
/// open project's export is stopped (another project's is refused by name).
#[tauri::command(async)]
pub(crate) fn export_cancel(
    state: State<'_, ExportState>,
    session: State<'_, ProjectSessionState>,
    job: Option<String>,
) -> Result<(), Said> {
    let open = open_feed(&session).ok().map(|feed| feed.project_id);
    state.cancel_for(job.as_deref(), open.as_deref())
}

/// The files of the last export at `index`, or all of them: only files it put in place (no other path is accepted).
fn exported(state: &ExportState, index: Option<usize>) -> Result<Vec<PathBuf>, Said> {
    let files = match index {
        Some(index) => state.file(index).into_iter().collect(),
        None => state.files(),
    };
    let here: Vec<PathBuf> = files.into_iter().filter(|f| f.is_file()).collect();
    if here.is_empty() {
        return Err(Said::code("exports.revealGone"));
    }
    Ok(here)
}

/// Shows file `index` of the last export in Finder, selected; without one, its first file: every file of an export is
/// in the same folder, and Finder selects only one of several it is asked to show.
#[tauri::command(async)]
pub(crate) fn export_reveal(
    state: State<'_, ExportState>,
    index: Option<usize>,
) -> Result<(), Said> {
    let files = exported(&state, index)?;
    system_open(true, &files[..1])
}

/// Opens file `index` of the last export in its app (QuickTime Player for a video).
#[tauri::command(async)]
pub(crate) fn export_open(state: State<'_, ExportState>, index: usize) -> Result<(), Said> {
    let files = exported(&state, Some(index))?;
    system_open(false, &files)
}

/// The last export's files in place, by index, with their sizes on the disk.
#[tauri::command(async)]
pub(crate) fn export_files(state: State<'_, ExportState>) -> Vec<ExportedFile> {
    state
        .files()
        .iter()
        .map(|path| ExportedFile {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            bytes: std::fs::metadata(path).map_or(0, |m| m.len()),
        })
        .collect()
}

/// Asks for the folder exports go to (Change); the dialog's name for it, or None when the person cancels.
#[tauri::command]
pub(crate) async fn export_pick_folder(
    app: AppHandle,
    state: State<'_, ExportState>,
) -> Result<Option<String>, Said> {
    let start = folder(&state).ok();
    let picker = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = picker.dialog().file().set_title("Save exports to");
        if let Some(dir) = start.filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        dialog.blocking_pick_folder()
    })
    .await
    .ok()
    .flatten();
    let Some(dir) = picked.and_then(|f| f.into_path().ok()) else {
        return Ok(None);
    };
    let label = export::folder_label(&dir);
    state.set_folder(dir);
    Ok(Some(label))
}

/// Finder shows `files` (`reveal`), or their apps open them.
fn system_open(reveal: bool, files: &[PathBuf]) -> Result<(), Said> {
    let mut command = opener(reveal, files);
    command.spawn().map(|_| ()).map_err(|error| {
        tracing::warn!(%error, "export: Finder couldn't be asked to show a file");
        Said::code("exports.revealGone")
    })
}

#[cfg(target_os = "macos")]
fn opener(reveal: bool, files: &[PathBuf]) -> std::process::Command {
    let mut command = std::process::Command::new("open");
    if reveal {
        command.arg("-R");
    }
    command.args(files);
    command
}

/// Elsewhere the file manager shows the first file's folder.
#[cfg(not(target_os = "macos"))]
fn opener(reveal: bool, files: &[PathBuf]) -> std::process::Command {
    let target = files
        .first()
        .map_or(std::path::Path::new("."), PathBuf::as_path);
    let shown = if reveal {
        target.parent().unwrap_or(target)
    } else {
        target
    };
    let mut command = std::process::Command::new("xdg-open");
    command.arg(shown);
    command
}

/// The app menu's Quit item, which asks Tauri to exit (so the guard below hears it).
#[cfg(target_os = "macos")]
const QUIT_ITEM: &str = "klipolo-quit";

/// Command Q reaches the quit guard: the standard menu's Quit ends a Mac app at once, with no exit request to hold, so
/// the app menu gets its own Quit (same name, same keys) that asks Tauri to exit instead. Elsewhere the window's close
/// is the way out, and it is guarded already.
#[cfg(target_os = "macos")]
pub(crate) fn guard_menu_quit(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, MenuItemKind};
    let menu = Menu::default(app.handle())?;
    if let Some(MenuItemKind::Submenu(first)) = menu.items()?.into_iter().next() {
        for item in first.items()? {
            if let MenuItemKind::Predefined(standard) = &item
                && standard.text()?.starts_with("Quit")
            {
                let name = standard.text()?;
                first.remove(standard)?;
                first.append(&MenuItem::with_id(
                    app,
                    QUIT_ITEM,
                    name,
                    true,
                    Some("CmdOrCtrl+Q"),
                )?)?;
            }
        }
    }
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if event.id() == QUIT_ITEM {
            app.exit(0);
        }
    });
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn guard_menu_quit(_app: &tauri::App) -> tauri::Result<()> {
    Ok(())
}

/// The quit guard: quitting (Command Q, or closing the window) while an export runs asks first. Keep exporting goes
/// back to it; Quit stops it (its job and parts deleted, as Cancel does), then quits. A quit that can't be asked about
/// still stops it and deletes its job on the way out.
pub(crate) fn guard_quit(app: &AppHandle, event: &RunEvent) {
    match event {
        RunEvent::ExitRequested { api, code, .. } => {
            if asks_first(app, code.unwrap_or(0)) {
                api.prevent_exit();
            }
        }
        RunEvent::WindowEvent {
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } => {
            if asks_first(app, 0) {
                api.prevent_close();
            }
        }
        // A quit nothing could ask before (the Dock's Quit, logging out, an AppleScript quit: a Mac app gets no exit
        // request for those): the running export stops and its job goes now, before the process ends.
        RunEvent::Exit => {
            if let Some(state) = app.try_state::<ExportState>() {
                export::clear_on_quit(&state, EXIT_WAIT);
            }
        }
        _ => {}
    }
}

/// True while an export runs and the person is being asked; its Quit exits with `exit_code`.
fn asks_first(app: &AppHandle, exit_code: i32) -> bool {
    let Some(state) = app.try_state::<ExportState>() else {
        return false;
    };
    if !state.is_running() || state.quitting() {
        return false;
    }
    if state.asking(true) {
        return true;
    }
    let handle = app.clone();
    app.dialog()
        .message("Quitting stops it, and the video being exported isn't saved.")
        .title("An export is running")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Quit".into(),
            "Keep exporting".into(),
        ))
        .show(move |quit| {
            let state = handle.state::<ExportState>();
            state.asking(false);
            if quit {
                state.set_quitting();
                tauri::async_runtime::spawn(stop_then_exit(handle.clone(), exit_code));
            }
        });
    true
}

/// Stops the running export and deletes its job here, waiting (briefly) for its run to let go of the slot, then quits:
/// nothing is left to a background delete the exit could cut off.
async fn stop_then_exit(app: AppHandle, exit_code: i32) {
    let handle = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        export::clear_on_quit(&handle.state::<ExportState>(), STOP_WAIT);
    })
    .await;
    app.exit(exit_code);
}

/// How long a quit nothing could hold (the Dock's Quit, logging out) waits for a stopped export to let go of its slot.
const EXIT_WAIT: Duration = Duration::from_secs(2);

/// The export state at launch, with the usual export folder swept of jobs a crash left (`export::sweep_folder`) on a
/// thread of its own.
pub(crate) fn state_at_launch() -> ExportState {
    if let Ok(dir) = export::default_folder() {
        let swept = std::thread::Builder::new()
            .name("klipolo-export-sweep".into())
            .spawn(move || export::sweep_folder(&dir));
        if let Err(error) = swept {
            tracing::warn!(%error, "export: the leftover jobs couldn't be swept");
        }
    }
    ExportState::default()
}
