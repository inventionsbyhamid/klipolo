//! The engine handle. It is cheap to clone. Seeks and preview ops travel on watch channels, so the newest value
//! always wins. Everything else goes through a command queue to the actor (`actor.rs`), which owns the backend.
//!
//! Lifecycle: the app starts the engine when the editor first attaches and keeps it for the app's lifetime. With no
//! project open, the backend is reset, so it holds no decoders or per-project GPU textures. The actor is supervised:
//! if it panics or loses its project feed, the UI gets an `Error` (the preview note with Retry, whatever the
//! presenter) and a native view still showing its last frame is taken out.

mod actor;
pub mod backend;
pub mod blank;
mod slot;

use std::sync::{Arc, Mutex, PoisonError};

use klipolo_model::Op;
use klipolo_model::project::Project;
use klipolo_model::speech::Speech;
use klipolo_model::time::Us;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::api::{
    CodedError, EngineEvent, EngineInfo, PREVIEW_STOPPED, Presenter, RATE_UNAVAILABLE, ViewSpec,
    rate_supported,
};
use crate::frames::server::FrameServer;
use crate::media::MediaKind;
use crate::plan::MediaMap;
use crate::platform::NativeSurface;
use crate::platform::gpu::GpuClass;
use crate::present::PresentLog;
pub use backend::{AudioTime, Pixels, PreviewBackend, RenderRequest, Rendered, RenderedRgba};
pub use slot::EngineSlot;

const STOPPED: &str = "The preview has stopped.";
/// What the UI shows when the engine stops on its own: the words the socket's closing shows (`SocketCanvas`).
pub const UNAVAILABLE: &str = "The preview isn't available right now.";

/// The open project as the project session publishes it; `None` when none is open.
#[derive(Clone, Debug)]
pub struct ProjectFeed {
    pub project_id: String,
    pub revision: u64,
    /// The project session's open count: create and open bump it, edits keep it. Revisions restart at 0 on every
    /// open, so a new count starts a new engine session even for the same project (frames and previews of the
    /// previous opening never match the new one).
    pub opened: u64,
    pub project: Arc<Project>,
    /// What was said, for captions. A finished transcript sends the same revision again with a new `speech` (the engine
    /// builds its pages again only when this isn't the same `Arc`).
    pub speech: Arc<Speech>,
    /// The language the project's video text is in (`klipolo_model::text_language::text_language`, worked out by the app):
    /// Chinese, Japanese and Korean draw in its faces. A switch of the interface language republishes the feed.
    pub text_language: Arc<str>,
    /// What contributions draw: the built-ins, the library's and the project's copies in the catalogue,
    /// and a shader only for what this computer checked (the app's snapshot, `make::snapshot`). The snapshot rebuilds it
    /// off the actor and sends the same revision again with a new `registry`, which the engine takes as a change (as it
    /// does a new `speech`: only when this isn't the same `Arc`).
    pub registry: Arc<crate::registry::Registry>,
}

pub type FeedRx = watch::Receiver<Option<ProjectFeed>>;
pub type EventSink = Arc<dyn Fn(EngineEvent) + Send + Sync>;

pub struct EngineOptions {
    pub allowed_origins: Vec<String>,
    pub max_preview_pixels: u32,
    /// Playback frames over the socket stay within this many pixels (`GpuBackend::socket_playback_pixels`).
    pub socket_playback_pixels: u32,
    /// Every socket frame RGBA, none NV12 (the trial's baseline).
    pub socket_rgba_only: bool,
    pub adapter: String,
    /// What kind of GPU the backend runs on (`GpuBackend::gpu_class`), reported in `EngineInfo`.
    pub gpu_class: GpuClass,
}

/// Dry-run ops the UI is previewing (a drag or a slider), valid only against `base_revision`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Transient {
    pub project_id: String,
    pub base_revision: u64,
    pub ops: Vec<Op>,
}

pub(crate) enum Cmd {
    View(ViewSpec),
    Visible(bool),
    Attached,
    Play {
        from_us: Us,
        /// One of `api::PLAYBACK_RATES` (checked by `Engine::play`).
        rate: f64,
        /// The run's sequence: its frames carry it (`Engine::play`).
        seq: u32,
        reply: oneshot::Sender<Result<(), CodedError>>,
    },
    Pause {
        reply: oneshot::Sender<Us>,
    },
    /// The UI's measured time from a frame's publish to it being drawn (ms).
    PresentDelay(f64),
    /// Present into this native view from now on.
    AttachNative {
        surface: NativeSurface,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Back to the socket.
    DetachNative,
    PresentLog(oneshot::Sender<Option<Arc<PresentLog>>>),
    /// The preview reads these sources from their editing copies from now on.
    Media(Arc<MediaMap>),
    /// The text layer being typed on the canvas, which the preview leaves out (`Engine::set_editing`), or none.
    Editing(Option<Arc<str>>),
    /// The app wrote this agent-made effect's crash-guard mark to disk (`EngineEvent::FirstDraw`'s answer).
    MarkWritten {
        contrib: klipolo_model::contrib::ContribId,
        seq: u64,
    },
    Shutdown(oneshot::Sender<()>),
}

#[derive(Clone)]
pub struct Engine {
    info: EngineInfo,
    cmds: mpsc::UnboundedSender<Cmd>,
    seek: Arc<watch::Sender<(Us, u32)>>,
    transient: Arc<watch::Sender<Option<Transient>>>,
    sink: Arc<Mutex<Option<EventSink>>>,
}

impl Engine {
    /// Binds the frame server and starts the actor. Nothing renders until a project and a view exist.
    pub async fn start<B: PreviewBackend>(
        options: EngineOptions,
        feed: FeedRx,
        backend: B,
    ) -> Result<Engine, String> {
        let (server, frames) = FrameServer::start(options.allowed_origins)
            .await
            .map_err(|e| format!("The preview couldn't start: {e}"))?;
        let info = EngineInfo {
            port: server.port,
            token: server.token.clone(),
            max_preview_pixels: options.max_preview_pixels,
            adapter: options.adapter,
            presenter: Presenter::Socket,
            gpu_class: options.gpu_class,
        };
        let (cmds, cmd_rx) = mpsc::unbounded_channel();
        let seek = Arc::new(watch::channel((0, 0)).0);
        let transient = Arc::new(watch::channel(None).0);
        let sink = Arc::new(Mutex::new(None));
        let actor = actor::Actor::new(actor::Parts {
            backend,
            _server: server,
            frames,
            feed,
            seek: seek.subscribe(),
            transient: transient.subscribe(),
            cmds: cmd_rx,
            sink: sink.clone(),
            max_pixels: options.max_preview_pixels,
            socket_playback_pixels: options.socket_playback_pixels,
            socket_rgba_only: options.socket_rgba_only,
        });
        supervise(tokio::spawn(actor.run()), sink.clone());
        Ok(Engine {
            info,
            cmds,
            seek,
            transient,
            sink,
        })
    }

    pub fn info(&self) -> EngineInfo {
        self.info.clone()
    }

    /// Events go here from now on (a reloaded webview attaches again). `Ready` is repeated for the open project.
    pub fn set_event_sink(&self, sink: EventSink) {
        *self.sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sink);
        let _ = self.cmds.send(Cmd::Attached);
    }

    pub fn set_view(&self, view: ViewSpec) {
        let _ = self.cmds.send(Cmd::View(view));
    }

    pub fn set_visible(&self, visible: bool) {
        let _ = self.cmds.send(Cmd::Visible(visible));
    }

    pub fn seek(&self, at_us: Us, seq: u32) {
        self.seek.send_replace((at_us, seq));
    }

    pub fn preview(&self, project_id: String, base_revision: u64, ops: Vec<Op>) {
        self.transient.send_replace(Some(Transient {
            project_id,
            base_revision,
            ops,
        }));
    }

    pub fn preview_clear(&self) {
        self.transient.send_replace(None);
    }

    /// While the person types a title on the canvas, the UI draws it there: the preview leaves out
    /// the text layer `item` (an overlay text, never the captions) until this is called with `None`, which the UI does
    /// once the edit's revision has landed, so nothing flashes. Export never sees it.
    pub fn set_editing(&self, item: Option<String>) {
        let _ = self.cmds.send(Cmd::Editing(item.map(Arc::from)));
    }

    /// Starts playback at `from_us` at `rate` (J/K/L shuttle: one of `api::PLAYBACK_RATES`, negative backward). Forward
    /// from the start when `from_us` is the last frame or later; backward it ends at 0. While playing, it changes the
    /// speed or direction from `from_us`. Sound plays at 1× only. Rust owns the clock: frames carry their time,
    /// and `State` events report starts and stops.
    ///
    /// `seq` names the run as a seek's does: its frames carry it, so the UI drops frames still in flight from the run
    /// before (a shuttle press while playing), and a seek sent before it is stale and ignored.
    ///
    /// Errors are codes the UI words: `RATE_UNAVAILABLE`, `PLAY_NO_PROJECT`, `PLAY_NO_VIDEO` and
    /// `PREVIEW_STOPPED`.
    pub async fn play(&self, from_us: Us, rate: f64, seq: u32) -> Result<(), CodedError> {
        if !rate_supported(rate) {
            return Err(CodedError::new(RATE_UNAVAILABLE));
        }
        let (reply, wait) = oneshot::channel();
        self.cmds
            .send(Cmd::Play {
                from_us,
                rate,
                seq,
                reply,
            })
            .map_err(|_| CodedError::new(PREVIEW_STOPPED))?;
        wait.await.map_err(|_| CodedError::new(PREVIEW_STOPPED))?
    }

    /// Stops playback and returns where it stopped (the frame on screen).
    pub async fn pause(&self) -> Us {
        let (reply, wait) = oneshot::channel();
        if self.cmds.send(Cmd::Pause { reply }).is_err() {
            return 0;
        }
        wait.await.unwrap_or(0)
    }

    /// How long the UI takes from a frame being sent to it being drawn (ms, recent median while playing). Frames are
    /// drawn that much ahead of the sound, so picture and sound line up.
    pub fn report_present_delay(&self, ms: f64) {
        let _ = self.cmds.send(Cmd::PresentDelay(ms));
    }

    /// Presents into `surface` (on macOS a layer made under the webview by `present::macos::install`) instead of the
    /// socket. An error leaves the socket in charge.
    pub async fn attach_native(&self, surface: NativeSurface) -> Result<(), String> {
        let (reply, wait) = oneshot::channel();
        self.cmds
            .send(Cmd::AttachNative { surface, reply })
            .map_err(|_| STOPPED.to_string())?;
        wait.await.map_err(|_| STOPPED.to_string())?
    }

    /// Frames go over the socket again (the UI asked for the compatibility preview).
    pub fn detach_native(&self) {
        let _ = self.cmds.send(Cmd::DetachNative);
    }

    /// The native presenter's present log, if it is presenting (the dev-only trial reads it).
    pub async fn present_log(&self) -> Option<Arc<PresentLog>> {
        let (reply, wait) = oneshot::channel();
        self.cmds.send(Cmd::PresentLog(reply)).ok()?;
        wait.await.ok().flatten()
    }

    /// The preview reads each source in `map` from its editing copy (same times, so nothing jumps), in place of the
    /// original; a source not in it is read as itself. Export never sees the map. Applied at once when stopped,
    /// at the end of playback otherwise (a decoder swap mid-play would stall a frame).
    pub fn set_media(&self, map: Arc<MediaMap>) {
        let _ = self.cmds.send(Cmd::Media(map));
    }

    /// The answer to `EngineEvent::FirstDraw { contrib, seq }`: that mark is written and synced to
    /// disk, so what it covers goes ahead (the compile, or drawing from the next frame). Only the app's crash guard sends
    /// it, after the mark landed.
    pub fn mark_written(&self, contrib: klipolo_model::contrib::ContribId, seq: u64) {
        let _ = self.cmds.send(Cmd::MarkWritten { contrib, seq });
    }

    /// Tells the UI that `path`'s derived media changed (`EngineEvent::Media`), for work done outside the engine (an
    /// editing copy refilled the filmstrip). Nothing happens before the UI has attached.
    pub fn media_event(&self, path: String, kind: MediaKind) {
        let sink = self
            .sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(sink) = sink {
            sink(EngineEvent::Media { path, kind });
        }
    }

    /// False once the actor has ended (shut down, or it panicked): the app then starts a new engine on attach.
    pub fn is_running(&self) -> bool {
        !self.cmds.is_closed()
    }

    /// Stops playback, drops decoders and closes the frame server.
    pub async fn shutdown(&self) {
        let (done, wait) = oneshot::channel();
        if self.cmds.send(Cmd::Shutdown(done)).is_ok() {
            let _ = wait.await;
        }
    }
}

/// Watches the actor. A panic, or the end of its project feed while the app still uses it, is reported: the UI gets
/// the `Error` the socket's closing would give (the preview note with Retry, which starts a new engine through
/// `EngineSlot`), and the native view, which would keep its last frame, is taken out.
fn supervise(actor: JoinHandle<actor::Exit>, sink: Arc<Mutex<Option<EventSink>>>) {
    tokio::spawn(async move {
        let reason = match actor.await {
            Ok(actor::Exit::Asked) => return,
            // The runtime is shutting down with the app.
            Err(e) if e.is_cancelled() => return,
            Ok(actor::Exit::Lost) => "its project feed closed".to_string(),
            Err(e) => {
                let panic = e.into_panic();
                panic
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "a panic".into())
            }
        };
        tracing::error!(%reason, "the preview engine stopped");
        crate::platform::present::uninstall_soon();
        let sink = sink.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(sink) = sink {
            sink(EngineEvent::Error {
                message: UNAVAILABLE.into(),
            });
        }
    });
}
