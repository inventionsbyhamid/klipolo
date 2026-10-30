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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_engine::api::{
    CodedError, EngineEvent, EngineInfo, PLAY_NO_PROJECT, PLAY_NO_VIDEO, PREVIEW_STOPPED,
    Presenter, RATE_UNAVAILABLE, ViewRect, ViewSpec,
};
use klipolo_engine::platform::NativeSurface;
use klipolo_engine::render::GpuBackend;
use klipolo_engine::session::{Engine, EngineOptions, EventSink, FeedRx};
use klipolo_model::time::Us;
use tauri::ipc::Channel;
use tauri::{State, WebviewWindow};

use crate::state::{EngineState, ProjectState, Transport};

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
/// layer from `host` when `setting` allows it and it starts, else the socket.
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
    let engine = state.slot.get_or_start(start).await?;
    engine.set_event_sink(sink);
    // A (re)loaded page numbers its seeks from 1 again, and the engine's count starts over on every attach: so does the
    // transport's, keeping its place.
    state
        .transport
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .seq = 0;
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

/// The app's engine: on the warm-up's GPU backend (or a new one), with the sound and thumbnail caches in the app's
/// cache folder.
async fn start_engine(state: &EngineState, feed: FeedRx) -> Result<Engine, String> {
    let mut backend = GpuBackend::for_preview(state.take_warm_up()).await?;
    if let Some(cache) = &state.cache_dir {
        backend = backend
            .with_audio_cache(cache.join("audio"))
            .with_media_cache(cache.join("media"))
            // Exports land later; until then no export job's files need keeping.
            .with_cache_tidy(None);
    }
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
    Ok(engine)
}

/// What the UI hears: every engine event except the crash guard's (Klipolo has no agent-made effects). The transport
/// follows playback's starts and stops; the log notes them, with the frames the native layer presented.
fn event_sink(events: Channel<EngineEvent>, transport: Arc<Mutex<Transport>>) -> EventSink {
    let presented = AtomicU64::new(0);
    Arc::new(move |event: EngineEvent| {
        if event.is_guard() {
            return;
        }
        match &event {
            EngineEvent::State {
                playing,
                at_us,
                reason,
                ..
            } => {
                transport
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .at_us = *at_us;
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

/// An engine code in plain words.
fn said(error: CodedError) -> String {
    match error.code.as_str() {
        PLAY_NO_PROJECT => "Open a video to play it.",
        PLAY_NO_VIDEO => "Add a video to play it.",
        RATE_UNAVAILABLE => "That playback speed isn't available.",
        PREVIEW_STOPPED => "The preview has stopped.",
        _ => "The preview couldn't do that.",
    }
    .to_string()
}

/// Starts the engine if needed and says how frames reach the screen. Events arrive on `events` from now on.
#[tauri::command]
pub async fn engine_attach(
    window: WebviewWindow,
    state: State<'_, EngineState>,
    projects: State<'_, ProjectState>,
    events: Channel<EngineEvent>,
) -> Result<EngineInfo, String> {
    let sink = event_sink(events, state.transport.clone());
    let setting = std::env::var(PRESENTER_SETTING).ok();
    let feed = projects.feed();
    let info = attach(
        &state,
        || start_engine(&state, feed),
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

/// Shows the frame at `at_us`. `seq` names the seek: frames carry it, and a seek older than the newest is ignored.
#[tauri::command]
pub fn engine_seek(state: State<'_, EngineState>, at_us: Us, seq: u32) {
    seek(&state, at_us, seq);
}

/// `engine_seek`: moves the transport and the engine to `at_us`, unless `seq` is older than the newest seek.
pub fn seek(state: &EngineState, at_us: Us, seq: u32) {
    let mut transport = state
        .transport
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if (seq.wrapping_sub(transport.seq) as i32) < 0 {
        return;
    }
    *transport = Transport { at_us, seq };
    if let Some(engine) = state.slot.running() {
        engine.seek(at_us, seq);
    }
}

/// Plays from where the transport is, at normal speed, with sound. Returns where it started (from the start again
/// after the end).
#[tauri::command]
pub async fn engine_play(state: State<'_, EngineState>) -> Result<Us, String> {
    let engine = state
        .slot
        .running()
        .ok_or_else(|| "The preview isn't ready yet.".to_string())?;
    let Transport { at_us, seq } = state.transport();
    engine.play(at_us, 1.0, seq).await.map_err(said)?;
    // The engine said where it started (its `State` event, before the reply): the sink moved the transport there.
    Ok(state.transport().at_us)
}

/// Pauses and returns where playback stopped.
#[tauri::command]
pub async fn engine_pause(state: State<'_, EngineState>) -> Result<Us, String> {
    let engine = state
        .slot
        .running()
        .ok_or_else(|| "The preview isn't ready yet.".to_string())?;
    let at_us = engine.pause().await;
    state
        .transport
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .at_us = at_us;
    Ok(at_us)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ProjectState;
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

    /// A reloaded page attaches again and numbers its seeks from 1, as the engine expects (it starts its count over on
    /// every attach): none of them may be dropped as older than the previous page's.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_reloaded_page_seeks_from_one_again() {
        let projects = ProjectState::default();
        projects.open(Project::new("Test"));
        let state = EngineState::new(None, None);
        let socket = Some("socket");
        attach(
            &state,
            || blank_engine(projects.feed()),
            &Detached,
            quiet(),
            socket,
        )
        .await
        .unwrap();
        seek(&state, 1_000_000, 9);
        assert_eq!(
            state.transport(),
            Transport {
                at_us: 1_000_000,
                seq: 9
            }
        );

        attach(
            &state,
            || blank_engine(projects.feed()),
            &Detached,
            quiet(),
            socket,
        )
        .await
        .unwrap();
        assert_eq!(state.transport().at_us, 1_000_000, "the place is kept");
        seek(&state, 2_000_000, 1);
        assert_eq!(
            state.transport(),
            Transport {
                at_us: 2_000_000,
                seq: 1
            },
            "the new page's first seek moves it"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn engine_info_reports_presenter() {
        let projects = ProjectState::default();
        projects.open(Project::new("Test"));
        let state = EngineState::new(None, None);

        let info = attach(
            &state,
            || test_engine(projects.feed()),
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
            || test_engine(projects.feed()),
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
