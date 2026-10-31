//! The preview engine's commands: thin adapters over `klipolo_engine::session::Engine`.
//!
//! Lifecycle: only `engine_attach` starts the engine, and it lives until the app exits (an engine whose actor ended is
//! replaced on the next attach). The other commands do nothing until then. The GPU warm-up compiles the pipelines in
//! the background from launch (`main`); the engine's first start takes its backend.
//!
//! Presenters: on macOS the preview is a native Metal layer under the transparent webview, unless
//! `KLIPOLO_PRESENTER=socket`; everywhere else, and whenever the native layer can't start, frames go over the
//! loopback websocket into the page's canvas.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use klipolo_engine::api::{
    EngineEvent, EngineInfo, Presenter, RATE_UNAVAILABLE, ViewRect, ViewSpec, rate_supported,
};
use klipolo_engine::platform::NativeSurface;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::session::{Engine, EngineOptions, EventSink, FeedRx};
use klipolo_model::Op;
use klipolo_model::coded::Said;
use klipolo_model::time::Us;
use tauri::ipc::Channel;
use tauri::{State, WebviewWindow};

use crate::media_jobs::MediaJobs;
use crate::project::ProjectSessionState;
use crate::state::EngineState;

/// The environment setting that picks the presenter: `socket` keeps frames on the websocket.
pub const PRESENTER_SETTING: &str = "KLIPOLO_PRESENTER";

/// Where a native preview's layer comes from: the app's window, or a test's layer in no window.
pub trait NativeHost {
    /// Makes the layer under the webview (or finds the one already there).
    fn install(&self) -> impl Future<Output = Result<NativeSurface, String>> + Send;
    /// Takes it out again: the socket presents.
    fn uninstall(&self);
}

/// The window's webview hosts the native layer (macOS); elsewhere there is none.
struct WindowHost<'a>(#[cfg_attr(not(target_os = "macos"), allow(dead_code))] &'a WebviewWindow);

impl NativeHost for WindowHost<'_> {
    #[cfg(target_os = "macos")]
    async fn install(&self) -> Result<NativeSurface, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.0
            .with_webview(move |webview| {
                let _ = tx.send(klipolo_engine::present::macos::install(webview.inner()));
            })
            .map_err(|e| e.to_string())?;
        let layer = rx.await.map_err(|_| "The window closed.".to_string())??;
        Ok(NativeSurface::Metal(layer))
    }

    #[cfg(not(target_os = "macos"))]
    async fn install(&self) -> Result<NativeSurface, String> {
        Err("This computer has no native preview.".into())
    }

    fn uninstall(&self) {
        #[cfg(target_os = "macos")]
        let _ = self
            .0
            .run_on_main_thread(klipolo_engine::present::macos::uninstall);
    }
}

/// Whether to present through the native layer: on macOS, unless the setting says `socket`.
pub fn native_wanted(setting: Option<&str>) -> bool {
    cfg!(target_os = "macos") && setting != Some("socket")
}

/// Starts the engine if needed (`start`), sends its events to `sink`, and says how frames reach the screen: the native
/// layer from `host` when `setting` allows it and it starts, else the socket. A page that attaches again (a reload)
/// starts paused, as the engine is then: playback from the page before stops, so sound never plays while the new page
/// shows Play.
pub async fn attach<S, Fut>(
    state: &EngineState,
    start: S,
    host: &impl NativeHost,
    sink: EventSink,
    setting: Option<&str>,
) -> Result<EngineInfo, String>
where
    S: FnOnce() -> Fut,
    Fut: Future<Output = Result<Engine, String>>,
{
    let again = state.slot.running().is_some();
    let engine = state.slot.get_or_start(start).await?;
    engine.set_event_sink(sink);
    if again {
        engine.pause().await;
    }
    let mut info = engine.info();
    if native_wanted(setting) {
        match present_natively(&engine, host).await {
            Ok(()) => info.presenter = Presenter::Native,
            Err(error) => {
                tracing::warn!(%error, "the native preview is unavailable; using the socket")
            }
        }
    } else {
        engine.detach_native();
        host.uninstall();
    }
    Ok(info)
}

/// Puts the native layer under the webview and hands it to the engine; on failure the layer is taken out again and
/// the socket stays in charge.
async fn present_natively(engine: &Engine, host: &impl NativeHost) -> Result<(), String> {
    let surface = host.install().await?;
    if let Err(error) = engine.attach_native(surface).await {
        host.uninstall();
        return Err(error);
    }
    Ok(())
}

/// Webview origins allowed to connect to the frame socket.
fn allowed_origins() -> Vec<String> {
    let mut origins = vec!["tauri://localhost".to_string()];
    if cfg!(debug_assertions) {
        origins.push("http://127.0.0.1:1420".into());
    }
    origins
}

/// The app's engine: on the warm-up's GPU backend (or a new one), with the app's sound cache (shared with analysis) and
/// the thumbnail cache in the app's cache folder. Editing copies attach to it (its GPU class, its thumbnail scans).
async fn start_engine(
    state: &EngineState,
    feed: FeedRx,
    copies: &MediaJobs,
) -> Result<Engine, String> {
    let mut backend = GpuBackend::for_preview(state.take_warm_up()).await?;
    if let (Some(caches), Some(sounds)) = (state.caches(), &state.sounds) {
        backend = backend
            .with_audio_store(sounds.clone())
            .with_media_cache(caches.media)
            // Exports read decoded sound from this cache too: a running job's files in the usual export folder are
            // kept (the open project's always are).
            .with_cache_tidy(
                klipolo_app::export::default_folder()
                    .ok()
                    .map(|dir| klipolo_engine::export::jobs::jobs_root(&dir)),
            );
    }
    let thumbs = backend.thumbs();
    let options = EngineOptions {
        allowed_origins: allowed_origins(),
        max_preview_pixels: backend.max_preview_pixels(),
        socket_playback_pixels: backend.socket_playback_pixels(),
        socket_rgba_only: false,
        adapter: backend.adapter_name(),
        gpu_class: backend.gpu_class(),
    };
    let engine = Engine::start(options, feed, backend).await?;
    tracing::info!(adapter = %engine.info().adapter, "the preview engine started");
    copies.attach(engine.clone(), thumbs);
    Ok(engine)
}

/// What the UI hears: every engine event except the crash guard's (Klipolo has no agent-made effects). Editing copies
/// hear them too (`copies`: when the video plays, a copy the preview couldn't open, a scan that may have ended). The
/// log notes playback's starts and stops, with the frames the native layer presented.
fn event_sink(events: Channel<EngineEvent>, copies: Option<MediaJobs>) -> EventSink {
    let presented = AtomicU64::new(0);
    Arc::new(move |event: EngineEvent| {
        if event.is_guard() {
            return;
        }
        if let Some(copies) = &copies {
            copies.on_engine_event(&event);
        }
        match &event {
            EngineEvent::State {
                playing,
                at_us,
                reason,
                ..
            } => {
                if *playing {
                    presented.store(0, Ordering::Relaxed);
                    tracing::info!(at_us, "playback started");
                } else {
                    let presented = presented.load(Ordering::Relaxed);
                    tracing::info!(at_us, ?reason, presented, "playback stopped");
                }
            }
            EngineEvent::Shown { presented: n, .. } => {
                presented.fetch_add(u64::from(*n), Ordering::Relaxed);
            }
            EngineEvent::Warning { message } | EngineEvent::Error { message } => {
                tracing::warn!(%message, "the preview says");
            }
            EngineEvent::Presenter { presenter } => {
                tracing::warn!(?presenter, "the preview changed presenter");
            }
            _ => {}
        }
        let _ = events.send(event);
    })
}

/// The engine hasn't started yet (the page attaches it).
fn not_ready() -> Said {
    Said::code("engine.notReady")
}

/// Starts the engine if needed and says how frames reach the screen. Events arrive on `events` from now on.
#[tauri::command]
pub async fn engine_attach(
    window: WebviewWindow,
    state: State<'_, EngineState>,
    projects: State<'_, ProjectSessionState>,
    copies: State<'_, MediaJobs>,
    events: Channel<EngineEvent>,
) -> Result<EngineInfo, String> {
    let sink = event_sink(events, Some(copies.inner().clone()));
    let setting = std::env::var(PRESENTER_SETTING).ok();
    let feed = projects.subscribe();
    let info = attach(
        &state,
        || start_engine(&state, feed, &copies),
        &WindowHost(&window),
        sink,
        setting.as_deref(),
    )
    .await
    .inspect_err(|error| tracing::warn!(%error, "the preview engine couldn't start"))?;
    tracing::info!(presenter = ?info.presenter, "the preview attached");
    Ok(info)
}

/// Where the native preview goes (the UI's preview hole), or null to hide it. Sync commands run on the main thread,
/// where the view lives, so it moves in the same turn; otherwise the move is queued there.
#[tauri::command]
pub fn engine_view(window: WebviewWindow, rect: Option<ViewRect>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if !klipolo_engine::present::macos::set_rect(rect) {
        window
            .run_on_main_thread(move || {
                klipolo_engine::present::macos::set_rect(rect);
            })
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (window, rect);
    Ok(())
}

/// The format and size (device pixels) the preview renders at.
#[tauri::command]
pub fn engine_set_view(state: State<'_, EngineState>, view: ViewSpec) {
    if let Some(engine) = state.slot.running() {
        engine.set_view(view);
    }
}

/// The window was hidden or minimised (`false`): the engine pauses with silence and lets its decoders go, until it
/// is shown again (`true`).
#[tauri::command]
pub fn engine_set_visible(state: State<'_, EngineState>, visible: bool) {
    set_visible(&state, visible);
}

/// `engine_set_visible`, for the running engine (nothing before the first attach).
pub fn set_visible(state: &EngineState, visible: bool) {
    if let Some(engine) = state.slot.running() {
        tracing::info!(visible, "the window's visibility changed");
        engine.set_visible(visible);
    }
}

/// Shows the frame at `at_us`. `seq` names the seek: frames carry it, and a seek older than the newest is ignored.
#[tauri::command]
pub fn engine_seek(state: State<'_, EngineState>, at_us: Us, seq: u32) {
    if let Some(engine) = state.slot.running() {
        engine.seek(at_us, seq);
    }
}

/// A dry run of `ops` on the open project `project_id` at `base_revision` (a drag, a slider): shown at once, never
/// committed. One for another project or revision is ignored.
#[tauri::command]
pub fn engine_preview(
    state: State<'_, EngineState>,
    project_id: String,
    base_revision: u64,
    ops: Vec<Op>,
) {
    if let Some(engine) = state.slot.running() {
        engine.preview(project_id, base_revision, ops);
    }
}

/// The dry run is over: the preview shows the project as it is.
#[tauri::command]
pub fn engine_preview_clear(state: State<'_, EngineState>) {
    if let Some(engine) = state.slot.running() {
        engine.preview_clear();
    }
}

/// While a title is typed on the canvas the page draws it there: the preview leaves out text layer `item` until this
/// is called with null, once the edit has landed (nothing flashes). Export never sees it.
#[tauri::command]
pub fn engine_set_editing(state: State<'_, EngineState>, item: Option<String>) {
    if let Some(engine) = state.slot.running() {
        engine.set_editing(item);
    }
}

/// Plays from `from_us` at `rate`: the J/K/L shuttle's -2, -1, 1 or 2 (1 when left out). Forward it starts over at the
/// end; backward it ends at 0. Sound plays at 1x only. `seq` names the run (its frames carry it, as a seek's do).
/// Errors are codes the UI words (`engine.play.noVideo`).
#[tauri::command]
pub async fn engine_play(
    state: State<'_, EngineState>,
    from_us: Us,
    rate: Option<f64>,
    seq: u32,
) -> Result<(), Said> {
    play(&state, from_us, rate, seq).await
}

/// `engine_play` on the running engine.
pub async fn play(
    state: &EngineState,
    from_us: Us,
    rate: Option<f64>,
    seq: u32,
) -> Result<(), Said> {
    let rate = rate.unwrap_or(1.0);
    if !rate_supported(rate) {
        return Err(Said::code(RATE_UNAVAILABLE));
    }
    let engine = state.slot.running().ok_or_else(not_ready)?;
    engine.play(from_us, rate, seq).await.map_err(Said::Coded)
}

/// Pauses and returns where playback stopped.
#[tauri::command]
pub async fn engine_pause(state: State<'_, EngineState>) -> Result<Us, Said> {
    Ok(state.slot.running().ok_or_else(not_ready)?.pause().await)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::project::opened;
    use klipolo_model::project::Project;

    /// A layer in no window: what a test's engine presents into on macOS.
    struct Detached;

    impl NativeHost for Detached {
        #[cfg(target_os = "macos")]
        async fn install(&self) -> Result<NativeSurface, String> {
            Ok(NativeSurface::Metal(
                klipolo_engine::present::macos::NativeLayer::detached(),
            ))
        }

        #[cfg(not(target_os = "macos"))]
        async fn install(&self) -> Result<NativeSurface, String> {
            Err("no native preview here".into())
        }

        fn uninstall(&self) {}
    }

    /// An engine on the feed: on macOS on the GPU (the native layer needs one), elsewhere on the blank backend.
    async fn test_engine(feed: FeedRx) -> Result<Engine, String> {
        #[cfg(target_os = "macos")]
        let backend =
            GpuBackend::new(klipolo_engine::render::mode::RenderMode::Interactive).await?;
        #[cfg(not(target_os = "macos"))]
        let backend = klipolo_engine::session::blank::BlankBackend::new();
        let options = EngineOptions {
            allowed_origins: allowed_origins(),
            max_preview_pixels: 1920 * 1080,
            socket_playback_pixels: 1920 * 1080,
            socket_rgba_only: false,
            adapter: "test".into(),
            gpu_class: klipolo_engine::platform::gpu::GpuClass::Integrated,
        };
        Engine::start(options, feed, backend).await
    }

    fn quiet() -> EventSink {
        Arc::new(|_| {})
    }

    /// An engine on the feed with no GPU (the socket presents).
    async fn blank_engine(feed: FeedRx) -> Result<Engine, String> {
        let options = EngineOptions {
            allowed_origins: allowed_origins(),
            max_preview_pixels: 1920 * 1080,
            socket_playback_pixels: 1920 * 1080,
            socket_rgba_only: false,
            adapter: "test".into(),
            gpu_class: klipolo_engine::platform::gpu::GpuClass::Integrated,
        };
        let backend = klipolo_engine::session::blank::BlankBackend::new();
        Engine::start(options, feed, backend).await
    }

    /// A project with one 10 s video on the main track (the blank backend never opens the file).
    fn one_clip() -> Project {
        use klipolo_model::project::{Asset, AssetKind};
        use klipolo_model::timeline::{Clip, ClipSource};
        let mut project = Project::new("Test");
        project.assets = vec![Asset {
            id: "v".into(),
            kind: AssetKind::Video,
            path: "/media/v.mp4".into(),
            name: "v.mp4".into(),
            duration_us: 10_000_000,
            width: 1920,
            height: 1080,
            fps: 30.0,
            rotation: Some(0),
            has_audio: false,
            audio_lead_us: Some(0),
            made_with: None,
        }];
        project.timeline.main = vec![Clip::new(
            ClipSource::Asset { id: "v".into() },
            0,
            10_000_000,
        )];
        project
    }

    /// Hidden or minimised, the window pauses playback (the engine lets its decoders go), and shown again it stays
    /// paused where it stopped.
    #[tokio::test(flavor = "multi_thread")]
    async fn hiding_the_window_pauses_playback() {
        let projects = opened(one_clip());
        let state = EngineState::new(None, None);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink: EventSink = {
            let seen = seen.clone();
            Arc::new(move |event| seen.lock().unwrap().push(event))
        };
        let socket = Some("socket");
        let engine = attach(
            &state,
            || blank_engine(projects.subscribe()),
            &Detached,
            sink,
            socket,
        );
        engine.await.unwrap();
        state
            .slot
            .running()
            .unwrap()
            .set_view(klipolo_engine::api::ViewSpec {
                format: klipolo_model::project::FormatKind::Landscape,
                width: 64,
                height: 36,
            });
        state.slot.running().unwrap().play(0, 1.0, 0).await.unwrap();

        set_visible(&state, false);
        let hidden = |e: &EngineEvent| {
            matches!(
                e,
                EngineEvent::State {
                    playing: false,
                    reason: klipolo_engine::api::StateReason::Hidden,
                    ..
                }
            )
        };
        for _ in 0..200 {
            if seen.lock().unwrap().iter().any(hidden) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            seen.lock().unwrap().iter().any(hidden),
            "playback stopped because the window was hidden"
        );
        set_visible(&state, true);
    }

    /// What the page hears: the crash guard's events never reach it, every other event does.
    #[test]
    fn the_sink_drops_guard_events() {
        use klipolo_engine::api::StateReason;
        use klipolo_model::contrib::ContribId;
        use tauri::ipc::InvokeResponseBody;

        let sent = Arc::new(Mutex::new(Vec::<String>::new()));
        let channel = {
            let sent = sent.clone();
            Channel::new(move |body| {
                if let InvokeResponseBody::Json(json) = body {
                    sent.lock().unwrap().push(json);
                }
                Ok(())
            })
        };
        let sink = event_sink(channel, None);

        sink(EngineEvent::FirstDraw {
            contrib: ContribId::builtin("effect.blur"),
            seq: 1,
        });
        sink(EngineEvent::DrewSafely {
            contrib: ContribId::builtin("effect.blur"),
            seq: 1,
        });
        assert!(
            sent.lock().unwrap().is_empty(),
            "guard events stay in the app"
        );

        sink(EngineEvent::State {
            playing: true,
            at_us: 2_000_000,
            reason: StateReason::Play,
            rate: 1.0,
        });
        sink(EngineEvent::Ready { session: 2 });

        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2, "{sent:?}");
        assert!(sent[0].contains(r#""type":"state""#) && sent[0].contains(r#""playing":true"#));
        assert!(sent[1].contains(r#""type":"ready""#));
    }

    /// A page that loads again while the video plays (a reload) finds it paused, as it shows it: the playback the page
    /// before started stops, and the new page hears so.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_reloaded_page_starts_paused() {
        let projects = opened(one_clip());
        let state = EngineState::new(None, None);
        let socket = Some("socket");
        attach(
            &state,
            || blank_engine(projects.subscribe()),
            &Detached,
            quiet(),
            socket,
        )
        .await
        .unwrap();
        let engine = state.slot.running().unwrap();
        engine.set_view(klipolo_engine::api::ViewSpec {
            format: klipolo_model::project::FormatKind::Landscape,
            width: 64,
            height: 36,
        });
        play(&state, 2_000_000, None, 1).await.unwrap();

        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink: EventSink = {
            let seen = seen.clone();
            Arc::new(move |event| seen.lock().unwrap().push(event))
        };
        attach(
            &state,
            || blank_engine(projects.subscribe()),
            &Detached,
            sink,
            socket,
        )
        .await
        .unwrap();
        let stopped = |e: &EngineEvent| matches!(e, EngineEvent::State { playing: false, .. });
        assert!(
            seen.lock().unwrap().iter().any(stopped),
            "the new page hears that playback stopped: {:?}",
            seen.lock().unwrap()
        );
        assert!(
            !seen
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, EngineEvent::State { playing: true, .. })),
            "nothing plays on"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn engine_info_reports_presenter() {
        let projects = opened(Project::new("Test"));
        let state = EngineState::new(None, None);

        let info = attach(
            &state,
            || test_engine(projects.subscribe()),
            &Detached,
            quiet(),
            None,
        )
        .await
        .unwrap();
        let native = if cfg!(target_os = "macos") {
            Presenter::Native
        } else {
            Presenter::Socket
        };
        assert_eq!(info.presenter, native, "the native layer is the default");
        assert_ne!(info.port, 0, "the socket stays up as the fallback");

        let info = attach(
            &state,
            || test_engine(projects.subscribe()),
            &Detached,
            quiet(),
            Some("socket"),
        )
        .await
        .unwrap();
        assert_eq!(
            info.presenter,
            Presenter::Socket,
            "KLIPOLO_PRESENTER=socket"
        );
    }
}
