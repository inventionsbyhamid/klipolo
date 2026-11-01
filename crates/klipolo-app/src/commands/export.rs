//! The export dialog's commands, thin layers over `klipolo_app::export`: plan the files with what they
//! cost, start an export on the one slot (its progress on a Channel), cancel it, open or show its files, pick the
//! folder they go to; list the unfinished exports, pause, resume and discard them; and the quit guard, which asks
//! before quitting pauses a running export to resume next time. Errors are codes the page words (`bridge/call.ts`).

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use klipolo_app::export::jobs::{self, Owner};
use klipolo_app::export::{
    self, ExportPlan, ExportState, ExportedFile, Releases, StartSteps, Started,
};
use klipolo_engine::api::{ExportEvent, ExportJobInfo, ExportQuality, ExportRequest, ResumeMode};
use klipolo_engine::export::job::ExportJob;
use klipolo_engine::export::quality;
use klipolo_engine::export::runner::{Batch, ProcessLauncher, discard_job};
use klipolo_engine::render::fonts;
use klipolo_engine::session::ProjectFeed;
use klipolo_model::coded::Said;
use klipolo_model::project::Project;
use serde::Deserialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, RunEvent, State, WebviewWindow, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

use crate::project::{PROJECT_EXTENSION, ProjectSessionState, library_dir, read};
use crate::state::EngineState;

/// How long a start waits for this Mac's fonts to be read before checking them: past it nothing is called missing,
/// and the worker still stops on a frame it would draw with a stand-in.
const FONTS_WAIT: Duration = Duration::from_secs(2);
/// How long Quit waits for a paused export to stop at its part and let go of its slot before the app goes anyway.
const PAUSE_WAIT: Duration = Duration::from_secs(10);

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

/// Where unfinished exports are kept: `.jobs` in the folder the person picked and in `Movies / Klipolo`.
fn job_roots(state: &ExportState) -> Vec<PathBuf> {
    jobs::roots(
        state.folder().as_deref(),
        export::default_folder().ok().as_deref(),
    )
}

/// This Mac has a media engine (asked once).
fn apple_silicon() -> bool {
    static FAST: OnceLock<bool> = OnceLock::new();
    *FAST.get_or_init(|| export::apple_silicon(&klipolo_platform::caps::current()))
}

/// This computer has a 10-bit HEVC encoder for Maximum (asked once, and only when a plan is for Maximum: the probe opens
/// a small encoder session, about 0.6 s on an Intel Mac).
fn hevc_main10() -> bool {
    static HEVC: OnceLock<bool> = OnceLock::new();
    *HEVC.get_or_init(|| quality::probe_caps_for(&[ExportQuality::Maximum]).hevc_main10)
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
        &feed.speech,
        &req,
        &folder(&state)?,
        apple_silicon(),
        req.quality != ExportQuality::Maximum || hevc_main10(),
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

    fn owner(&self, job: &str) -> Owner {
        jobs::owner(&job_roots(&self.app.state::<ExportState>()), job)
    }

    fn create(&self, batch: Batch) -> impl Future<Output = Result<ExportJob, Said>> + Send {
        export::create_job(batch)
    }

    fn discard(&self, job: &str) -> Result<(), Said> {
        jobs::discard(&job_roots(&self.app.state::<ExportState>()), job)
    }

    fn drop_new(&self, job: &ExportJob) {
        if let Err(why) = discard_job(&job.dir) {
            tracing::warn!(%why, "export: a job written for a start that didn't run couldn't be deleted");
        }
    }
}

/// Saves the latest edits (the export reads the project as saved), then freezes the open project into a batch for
/// `dir`: refused when it has nothing to show or uses a font that isn't on this Mac, and when the folder can't be made
/// or written in.
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
    // A folder Klipolo can't write in is named now, rather than failing as the job is written.
    export::make_folder(&dir)?;
    // What a crash cut off while a job was written or discarded goes now; kept jobs stay (`export::tidy_jobs`).
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
        pcm_dir,
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

/// What a run does: a new job (written as it started), or a kept job continued from its finished parts.
enum Work {
    Start(Box<ExportJob>),
    Resume(PathBuf, ResumeMode),
}

/// Runs `work` in the background on the slot it holds, telling the page on `events`, and lets go of the slot when it
/// ends.
fn spawn_run(
    app: &AppHandle,
    launcher: ProcessLauncher,
    work: Work,
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
        let outcome = match work {
            Work::Start(job) => export::run(&launcher, *job, cancel, &emit).await,
            Work::Resume(dir, mode) => export::resume(&launcher, dir, mode, cancel, &emit).await,
        };
        tracing::info!(?outcome, "an export ended");
    });
}

/// The export worker: this app's own binary.
fn launcher() -> Result<ProcessLauncher, Said> {
    std::env::current_exe()
        .map(ProcessLauncher::export)
        .map_err(|_| Said::code("exports.notStarted"))
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
    let launcher = launcher()?;
    let steps = AppSteps {
        app: &app,
        dir: folder(&state)?,
    };
    match export::start_on_slot(&state, &req, &steps).await? {
        Started::Run(job, cancel) => spawn_run(&app, launcher, Work::Start(job), cancel, events),
        Started::Cancelled => {
            let _ = events.send(ExportEvent::Cancelled);
        }
    }
    Ok(())
}

/// Unfinished exports, newest first, with their progress and whether their project changed since (`judge`).
#[tauri::command(async)]
pub(crate) fn export_jobs(
    state: State<'_, ExportState>,
    session: State<'_, ProjectSessionState>,
) -> Vec<ExportJobInfo> {
    let mut judge = Judge::new(open_feed(&session).ok());
    jobs::list(&job_roots(&state), |job| judge.stale(job))
}

/// What unfinished jobs are judged by: their project as it is now, looked up once per project however many of its
/// jobs are listed. The open project (its latest edits) with what was said in its files; another project from its file
/// in the library, found by the id stored in it, with the words its job froze (what was said in a project's files is
/// heard again only while it is open). A project that's gone counts as changed.
struct Judge {
    open: Option<ProjectFeed>,
    library: Option<PathBuf>,
    saved: HashMap<String, Option<Project>>,
}

impl Judge {
    fn new(open: Option<ProjectFeed>) -> Self {
        Self {
            open,
            library: library_dir().ok(),
            saved: HashMap::new(),
        }
    }

    /// `job`'s project changed since it started, its caption pages included, or is gone.
    fn stale(&mut self, job: &ExportJob) -> bool {
        if let Some(feed) = self
            .open
            .as_ref()
            .filter(|f| f.project.id == job.project_id)
        {
            let speech = export::export_speech(&feed.speech, &feed.project);
            return !job.is_current(&feed.project, &speech);
        }
        let library = self.library.as_deref();
        self.saved
            .entry(job.project_id.clone())
            .or_insert_with(|| library.and_then(|dir| saved_project(dir, &job.project_id)))
            .as_ref()
            .is_none_or(|project| !job.is_current_as_saved(project))
    }
}

/// The project with id `id` as saved in the library `dir` (its `.klipolo` files, read until one has it).
fn saved_project(dir: &Path, id: &str) -> Option<Project> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == PROJECT_EXTENSION))
        .filter_map(|path| read(&path).ok())
        .find(|project| project.id == id)
}

/// Pauses the running export `job`: it stops at its next part, keeping every finished part.
#[tauri::command(async)]
pub(crate) fn export_pause(state: State<'_, ExportState>, job: String) -> Result<(), Said> {
    state.pause(&job)
}

/// Continues the unfinished export `job` from its finished parts; progress arrives on `events` (as for `export_start`).
/// A job whose project changed since still resumes: it finishes the version from when the export started. `mode`
/// `compatible` finishes it in compatibility mode (offered after hardware failures); the default is `auto`. Refused as
/// busy while another export runs or this one runs already.
#[tauri::command]
pub(crate) async fn export_resume(
    app: AppHandle,
    state: State<'_, ExportState>,
    job: String,
    mode: Option<ResumeMode>,
    events: Channel<ExportEvent>,
) -> Result<(), Said> {
    let dir = jobs::find(&job_roots(&state), &job)?.ok_or_else(|| Said::code("exports.gone"))?;
    let launcher = launcher()?;
    let cancel = export::resume_on_slot(&state, &dir, &job).await?;
    spawn_run(
        &app,
        launcher,
        Work::Resume(dir, mode.unwrap_or_default()),
        cancel,
        events,
    );
    Ok(())
}

/// Deletes the unfinished export `job` and its parts (a running one is stopped first).
#[tauri::command(async)]
pub(crate) fn export_discard(state: State<'_, ExportState>, job: String) -> Result<(), Said> {
    jobs::discard(&job_roots(&state), &job)
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

/// A box in the page, in CSS px from its top left (the Share… button's `getBoundingClientRect`).
#[derive(Clone, Copy, Debug, Deserialize)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct PageRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Shows the Mac's share menu (AirDrop, Messages, Mail and the person's other apps) for the last export's files, beside
/// `anchor` (the Share… button): only files it put in place that are still there (`exports.revealGone` when none is),
/// and `exports.shareFailed` when the menu can't show. Resolves once the menu shows.
#[tauri::command]
pub(crate) async fn export_share(
    window: WebviewWindow,
    state: State<'_, ExportState>,
    anchor: PageRect,
) -> Result<(), Said> {
    let files = exported(&state, None)?;
    share(&window, anchor, files).await
}

/// The menu shows on the main thread, at the button's place in the webview (the page starts at its title bar inset).
#[cfg(target_os = "macos")]
async fn share(window: &WebviewWindow, anchor: PageRect, files: Vec<PathBuf>) -> Result<(), Said> {
    use klipolo_platform::share::{Anchor, show};
    let (tx, rx) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |webview| {
            let view = webview.inner();
            let (x, y) = klipolo_engine::present::macos::page_origin_in(view).unwrap_or_default();
            let at = Anchor {
                x: x + anchor.x,
                y: y + anchor.y,
                width: anchor.width,
                height: anchor.height,
            };
            let _ = tx.send(show(view, at, &files));
        })
        .map_err(|_| Said::code("exports.shareFailed"))?;
    match rx.await {
        Ok(true) => Ok(()),
        _ => Err(Said::code("exports.shareFailed")),
    }
}

/// No share menu here (Klipolo's builds for Linux and Windows are for its checks).
#[cfg(not(target_os = "macos"))]
async fn share(
    _window: &WebviewWindow,
    _anchor: PageRect,
    _files: Vec<PathBuf>,
) -> Result<(), Said> {
    Err(Said::code("exports.shareFailed"))
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

/// `open` for `files` (`reveal`: shown in Finder, selected, instead of opened in their app).
#[cfg(target_os = "macos")]
pub(crate) fn opener(reveal: bool, files: &[PathBuf]) -> std::process::Command {
    let mut command = std::process::Command::new("open");
    if reveal {
        command.arg("-R");
    }
    command.args(files);
    command
}

/// Elsewhere the file manager shows the first file's folder.
#[cfg(not(target_os = "macos"))]
pub(crate) fn opener(reveal: bool, files: &[PathBuf]) -> std::process::Command {
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
/// back to it; Quit pauses it (its finished parts kept, to resume next time), then quits. A quit that can't be asked
/// about pauses it too on the way out (`on_exit`). True when it held the exit or the close.
pub(crate) fn guard_quit(app: &AppHandle, event: &RunEvent) -> bool {
    match event {
        RunEvent::ExitRequested { api, code, .. } => {
            let held = asks_first(app, code.unwrap_or(0));
            if held {
                api.prevent_exit();
            }
            held
        }
        RunEvent::WindowEvent {
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } => {
            let held = asks_first(app, 0);
            if held {
                api.prevent_close();
            }
            held
        }
        _ => false,
    }
}

/// The exit's two steps, in order: `flush` puts every edit onto the disk, then `pause` pauses a running export, its
/// finished parts kept (waiting up to `EXIT_WAIT` for it). The edits go first: macOS gives a quitting app little time,
/// and they are what the person would lose.
fn exit_steps(flush: impl FnOnce(), pause: impl FnOnce()) {
    flush();
    pause();
}

/// As the app exits (`RunEvent::Exit`), however it quits: the open project's edits are saved, then a running export
/// pauses, to resume next time, before the process ends. A quit nothing could ask before (the Dock's Quit, logging out,
/// the Mac shutting down, an AppleScript quit: a Mac app gets no exit request for those) reaches only this. Past the
/// wait the worker stops with the app, and its job still resumes from its finished parts.
pub(crate) fn on_exit(app: &AppHandle) {
    exit_steps(
        || {
            if let Err(error) = app.state::<ProjectSessionState>().flush() {
                tracing::warn!(
                    detail = error.detail(),
                    "the last edits couldn't be saved at quit"
                );
            }
        },
        || {
            if let Some(state) = app.try_state::<ExportState>() {
                export::pause_on_quit(&state, EXIT_WAIT);
            }
        },
    );
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
        .message("Quit now and resume it next time?")
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
                tauri::async_runtime::spawn(pause_then_exit(handle.clone(), exit_code));
            }
        });
    true
}

/// Pauses the running export, waiting (briefly) for it to stop at its part and let go of the slot, then quits. Past the
/// wait the app quits anyway: its worker stops with it, and the job resumes from its finished parts.
async fn pause_then_exit(app: AppHandle, exit_code: i32) {
    let handle = app.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || {
        export::pause_on_quit(&handle.state::<ExportState>(), PAUSE_WAIT);
    })
    .await;
    app.exit(exit_code);
}

/// How long a quit nothing could hold (the Dock's Quit, logging out, a shutdown) waits for a paused export to let go
/// of its slot.
const EXIT_WAIT: Duration = Duration::from_secs(2);

/// The export state at launch, the usual export folder swept of what a crash cut off while a job was written or
/// discarded (`export::sweep_folder`: kept jobs stay to resume), on a thread of its own.
pub(crate) fn state_at_launch() -> ExportState {
    if let Ok(dir) = export::default_folder() {
        let swept = std::thread::Builder::new()
            .name("klipolo-export-sweep".into())
            .spawn(move || export::sweep_folder(&dir));
        if let Err(error) = swept {
            tracing::warn!(%error, "export: the export folder couldn't be swept");
        }
    }
    ExportState::default()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::exit_steps;

    #[test]
    fn at_exit_the_edits_are_saved_before_the_export_is_paused() {
        let done = RefCell::new(Vec::new());
        exit_steps(
            || done.borrow_mut().push("flush"),
            || done.borrow_mut().push("pause"),
        );
        assert_eq!(done.into_inner(), ["flush", "pause"]);
    }
}
