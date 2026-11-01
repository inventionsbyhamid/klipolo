//! The engine actor: one task owns the backend and turns the project feed, the view, seeks and preview ops into
//! frames. Every input is "newest wins", so a slow render never queues stale work.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use klipolo_model::captions::Captions;
use klipolo_model::project::Project;
use klipolo_model::speech::Speech;
use klipolo_model::time::{RangeUs, Us};
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{mpsc, watch};

use super::{Cmd, EventSink, FeedRx, PreviewBackend, Transient};
use crate::api::{EngineEvent, Presenter, StateReason, ViewSpec};
use crate::audio::plan::{AudioPlan, audio_plan, ducking_speech_after};
use crate::frames::server::{FrameServer, FrameSink};
use crate::media::cursor::CursorTracks;
use crate::plan::{MediaMap, PlanContext};
use crate::present::{FramePresenter, PresentLog, PresenterKind, SocketPresenter};
use feed::{redrawn, zoom_done};
use level::{Leveller, level_done, level_due};
use playback::{Playing, next_tick};
use render::{InFlight, wait_front, within_budget};
use waiting::{NoteChange, Waiting, waiting_due};

mod feed;
mod level;
mod native;
mod playback;
mod render;
mod sound;
mod waiting;

/// Frames are drawn at most this far ahead of the sound (the measured delivery delay is clamped to it).
const MAX_LEAD_US: Us = 100_000;
/// One display refresh: a drawn frame reaches the screen on the next one.
const VSYNC_US: f64 = 16_667.0;

/// How the actor ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Exit {
    /// Shut down, or every engine handle is gone.
    Asked,
    /// Its project feed closed while the app still uses it: the preview can't go on.
    Lost,
}

pub(crate) struct Parts<B> {
    pub backend: B,
    /// Held for its `Drop`: the socket closes when the actor ends.
    pub _server: FrameServer,
    pub frames: FrameSink,
    pub feed: FeedRx,
    pub seek: watch::Receiver<(Us, u32)>,
    pub transient: watch::Receiver<Option<Transient>>,
    pub cmds: mpsc::UnboundedReceiver<Cmd>,
    pub sink: Arc<Mutex<Option<EventSink>>>,
    /// Preview frames never exceed this many pixels.
    pub max_pixels: u32,
    /// Playback frames sent over the socket never exceed this many (960×540 on Intel: the transport's cost scales
    /// with bytes); a pause draws the still at the full budget again.
    pub socket_playback_pixels: u32,
    /// Socket frames are all RGBA (the trial's "before" baseline) instead of NV12 while playing.
    pub socket_rgba_only: bool,
}

struct Current {
    project_id: String,
    revision: u64,
    opened: u64,
    committed: Arc<Project>,
    /// The feed's speech, and the committed project's caption pages built from it once per revision that may page
    /// differently (or per new speech).
    speech: Arc<Speech>,
    captions: Arc<Captions>,
    /// The committed project's sound, planned once per revision.
    sound: Arc<AudioPlan>,
    /// Where someone speaks on the committed project, which its ducking sound layers go down under
    /// (`audio::plan::ducking_speech`): once per revision and speech; a dry run that can't move it uses it again.
    speech_ranges: Arc<[RangeUs]>,
    /// The committed project, or its dry run with the UI's preview ops.
    ctx: PlanContext,
    /// Whether `ctx` sounds like the committed project: no dry run, or one whose ops can't change the sound
    /// (`audio::plan::may_change_sound`).
    quiet: bool,
}

impl Current {
    /// What `ctx` plays: the committed revision's plan when the dry run can't change it (no replan), else planned now,
    /// ducking under the speech where the dry run puts it.
    fn sound(&self) -> Arc<AudioPlan> {
        if self.quiet {
            self.sound.clone()
        } else {
            let p = &self.ctx.project;
            let speech =
                ducking_speech_after(p, &self.speech, &self.committed, &self.speech_ranges);
            Arc::new(audio_plan(p, &self.ctx.registry, speech))
        }
    }
}

pub(crate) struct Actor<B> {
    p: Parts<B>,
    view: Option<ViewSpec>,
    visible: bool,
    session: u32,
    current: Option<Current>,
    /// The UI's preview ops, taken only when the transient watch changes and dropped whenever the project id,
    /// revision or opening changes (so an old `(project, revision)` preview never matches a reopened project).
    transient: Option<Transient>,
    playhead_us: Us,
    seq: u32,
    dirty: bool,
    playing: Option<Playing>,
    last_error: Option<String>,
    warned: HashSet<String>,
    /// Sources already said to be missing this session (`api::MEDIA_MISSING`).
    missing_said: HashSet<Arc<str>>,
    /// Sources whose disk the latest frame waited for, and the note naming one (`api::WAITING_FOR_DISK`).
    waiting: Waiting,
    /// Frames submitted to the GPU whose pixels haven't arrived yet, oldest first (published in order).
    inflight: VecDeque<InFlight>,
    /// The backend's "an image, LUT or font finished loading" signal: the still is drawn again.
    redraw: Option<Arc<tokio::sync::Notify>>,
    /// Zoom springs too long to simulate on the actor, running on a blocking thread (newest edit wins).
    zoom_job: Option<feed::ZoomJob>,
    /// Scratch for the playback lookahead's clip starts (reused every frame).
    starts: Vec<Us>,
    /// Play was pressed before the preview had a size: playback starts here, at this rate, once it does.
    play_on_view: Option<(Us, f64)>,
    /// The backend's "the sound is on the clock, or the device failed" signal.
    audio_wake: Option<Arc<tokio::sync::Notify>>,
    /// How far ahead of the sound frames are drawn: this engine's tick → publish time plus the UI's publish →
    /// drawn time (both smoothed) plus one refresh, so a frame is on screen as its sound is heard.
    lead_us: Us,
    pipeline_us: f64,
    present_us: f64,
    /// When the playback frame being submitted was planned.
    ticked: Option<Instant>,
    level: Leveller,
    /// Where frames go: the socket, or the native layer once the app attached one.
    presenter: Box<dyn FramePresenter>,
    /// The native presenter's trial log (dev builds read it).
    present_log: Option<Arc<PresentLog>>,
    /// The editing copies the preview reads, in every context it plans with; and a newer map that arrived
    /// while playing, applied when playback stops.
    media: Arc<MediaMap>,
    pending_media: Option<Arc<MediaMap>>,
    /// The recordings' cursor tracks Auto zooms follow, loaded on their own thread as a project is
    /// published, and its "a track loaded" signal: the zooms are planned again.
    cursors: Arc<CursorTracks>,
    cursor_wake: Option<Arc<tokio::sync::Notify>>,
    /// The screen activity store's "a track grew" signal: the takes' facts and follow paths are built again.
    activity_wake: Option<Arc<tokio::sync::Notify>>,
    /// A take's new facts or paths arrived while playing: planned at the next pause (no jump mid-play).
    cursor_due: bool,
    /// The seek (its sequence) the still being drawn shows, or `NO_STILL`: the backend's `Superseded` compares it
    /// with the newest seek, so a scrub that moved on stops the still's reads early.
    drawing_still: Arc<AtomicU64>,
    /// The text layer typed on the canvas, left out of the preview's frames (`render::hide_edited`).
    editing: Option<Arc<str>>,
    /// The registry last given to the backend (`PreviewBackend::prepare_registry`).
    prepared: Option<Arc<crate::registry::Registry>>,
    /// When the backend's next crash-guard mark runs out of time.
    guard_due: Option<tokio::time::Instant>,
    /// The backend's "the GPU finished a frame a crash-guard mark waits for" signal.
    guard_wake: Option<Arc<tokio::sync::Notify>>,
}

/// `drawing_still` while no still is being drawn.
const NO_STILL: u64 = u64::MAX;

impl<B: PreviewBackend> Actor<B> {
    pub(crate) fn new(mut p: Parts<B>) -> Self {
        let redraw = p.backend.redraw();
        let audio_wake = p.backend.audio_wake();
        let guard_wake = p.backend.guard_wake();
        // A still's reads stop early once a newer seek arrived (a scrub moved on); playback frames never do.
        let drawing_still = Arc::new(AtomicU64::new(NO_STILL));
        let (still, seeks) = (drawing_still.clone(), p.seek.clone());
        p.backend.set_superseded(Arc::new(move || {
            let drawing = still.load(Ordering::Relaxed);
            drawing != NO_STILL && drawing != u64::from(seeks.borrow().1)
        }));
        let presenter = socket_presenter(&p.frames, p.socket_rgba_only);
        let cursors = Arc::new(CursorTracks::default());
        let cursor_wake = Some(cursors.wake());
        p.backend.set_cursors(cursors.clone());
        let activity_wake = feed::read_activity(&cursors, p.backend.activity());
        // A file's peaks or thumbnails are ready, or its sound is damaged: the UI hears it as an engine event.
        let sink = p.sink.clone();
        p.backend
            .set_media_sink(Arc::new(move |e: crate::media::MediaEvent| {
                let sink = sink.lock().unwrap_or_else(PoisonError::into_inner).clone();
                if let Some(sink) = sink {
                    sink(media_event(e));
                }
            }));
        Self {
            p,
            view: None,
            visible: true,
            session: 0,
            current: None,
            transient: None,
            playhead_us: 0,
            seq: 0,
            dirty: false,
            playing: None,
            last_error: None,
            warned: HashSet::new(),
            missing_said: HashSet::new(),
            waiting: Waiting::default(),
            inflight: VecDeque::new(),
            redraw,
            zoom_job: None,
            starts: Vec::new(),
            play_on_view: None,
            audio_wake,
            lead_us: 0,
            pipeline_us: 0.0,
            present_us: 0.0,
            ticked: None,
            level: Leveller::default(),
            presenter,
            present_log: None,
            media: Arc::default(),
            pending_media: None,
            cursors,
            cursor_wake,
            activity_wake,
            cursor_due: false,
            drawing_still,
            editing: None,
            prepared: None,
            guard_due: None,
            guard_wake,
        }
    }

    /// Reads sources through `map`'s editing copies from the next frame (same times: nothing jumps).
    pub(super) fn apply_media(&mut self, map: Arc<MediaMap>) {
        if Arc::ptr_eq(&map, &self.media) {
            return;
        }
        self.media = map;
        if let Some(cur) = self.current.as_mut() {
            cur.ctx.media = self.media.clone();
            self.dirty = true;
        }
    }

    /// Folds a new delivery measurement into the lead (smoothed over about a second of frames).
    fn update_lead(&mut self) {
        let lead = self.pipeline_us + self.present_us + VSYNC_US;
        self.lead_us = (lead.round() as Us).clamp(0, MAX_LEAD_US);
    }

    pub(crate) async fn run(mut self) -> Exit {
        self.on_feed();
        loop {
            tokio::select! {
                // The feed first: the project session publishes before its command returns, so any seek, preview
                // or command the UI sends afterwards must see that revision (an unbiased pick could clamp a seek
                // against the old timeline, or apply it before a project switch resets the playhead).
                biased;
                changed = self.p.feed.changed() => {
                    if changed.is_err() { return Exit::Lost; }
                    self.on_feed();
                }
                cmd = self.p.cmds.recv() => {
                    if !self.handle(cmd) { return Exit::Asked; }
                }
                // A frame's pixels arrived from the GPU: publish before taking more work, so the newest picture
                // reaches the UI as soon as it exists.
                frame = wait_front(&mut self.inflight) => self.publish_front(frame),
                // The engine handles own these watches: they close only when every handle is gone.
                changed = self.p.transient.changed() => {
                    if changed.is_err() { return Exit::Asked; }
                    self.on_transient();
                }
                changed = self.p.seek.changed() => {
                    if changed.is_err() { return Exit::Asked; }
                    self.on_seek();
                }
                () = next_tick(&self.playing) => self.on_tick().await,
                // While playing, the next tick draws the loaded asset anyway.
                () = redrawn(&self.redraw) => self.dirty |= self.playing.is_none(),
                () = redrawn(&self.audio_wake) => self.on_audio(),
                () = redrawn(&self.cursor_wake) => self.on_cursor(),
                () = redrawn(&self.activity_wake) => self.on_activity(),
                track = zoom_done(&mut self.zoom_job) => self.on_zoom(track),
                () = waiting_due(self.waiting.due()) => {
                    let note = self.waiting.update(tokio::time::Instant::now());
                    self.note(note);
                }
                () = level_due(self.level.due()) => self.start_level(),
                () = guard_due(self.guard_due) => {
                    self.p.backend.guard_tick();
                    self.emit_guard_events();
                }
                // A frame a draw mark waits for is done on the GPU: its 2 s start now.
                () = redrawn(&self.guard_wake) => {
                    self.p.backend.guard_tick();
                    self.emit_guard_events();
                }
                gain = level_done(&mut self.level) => self.on_level(gain),
            }
            if !self.drain() {
                return Exit::Asked;
            }
            if self.presenter.failed() {
                self.fall_back();
            }
            // While playing, every tick draws the newest state anyway.
            if self.dirty && self.playing.is_none() {
                self.render_still().await;
            }
        }
    }
}

impl<B: PreviewBackend> Actor<B> {
    /// Applies everything already waiting, in the same order as `run` (feed first), so a burst of resizes, seeks
    /// or preview ops renders one frame for the newest state instead of one per message. False: stop the actor.
    fn drain(&mut self) -> bool {
        loop {
            if self.p.feed.has_changed().unwrap_or(false) {
                self.on_feed();
                continue;
            }
            match self.p.cmds.try_recv() {
                Ok(cmd) => {
                    if !self.handle(Some(cmd)) {
                        return false;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return self.handle(None),
            }
        }
        if self.p.transient.has_changed().unwrap_or(false) {
            self.on_transient();
        }
        if self.p.seek.has_changed().unwrap_or(false) {
            self.on_seek();
        }
        true
    }

    /// False when the actor must stop (shutdown, or every engine handle is gone).
    fn handle(&mut self, cmd: Option<Cmd>) -> bool {
        match cmd {
            Some(Cmd::Shutdown(done)) => {
                self.stop_playing(self.playhead_us, StateReason::Pause);
                self.inflight.clear();
                self.p.backend.reset();
                let _ = done.send(());
                false
            }
            Some(cmd) => {
                self.on_cmd(cmd);
                true
            }
            None => {
                self.p.backend.stop_audio();
                self.inflight.clear();
                self.p.backend.reset();
                false
            }
        }
    }

    /// A play waiting for the preview's size is called off, and says it stopped at `at_us` (where it would have
    /// started, when None).
    fn cancel_queued_play(&mut self, reason: StateReason, at_us: Option<Us>) {
        if let Some((from_us, _)) = self.play_on_view.take() {
            self.emit(EngineEvent::State {
                playing: false,
                at_us: at_us.unwrap_or(from_us),
                reason,
                rate: 1.0,
            });
        }
    }

    /// The waiting note changed: the UI hears the new one, or that it goes.
    fn note(&self, change: NoteChange) {
        if let Some(notice) = change {
            self.emit(EngineEvent::Waiting { notice });
        }
    }

    fn emit(&self, event: EngineEvent) {
        let sink = self
            .p
            .sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(sink) = sink {
            sink(event);
        }
    }

    fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::View(view) => {
                // A dry run's pages are made for the previewed format only: another format makes its own.
                let reformatted = self.view.is_some_and(|v| v.format != view.format);
                let (width, height) = within_budget(view.width, view.height, self.p.max_pixels);
                self.view = Some(ViewSpec {
                    width,
                    height,
                    ..view
                });
                self.dirty = true;
                if reformatted && self.transient.is_some() {
                    self.refresh();
                }
                if let Some((from, rate)) = self.play_on_view.take() {
                    self.start_playing(from, rate);
                }
            }
            Cmd::PresentDelay(ms) => {
                if ms.is_finite() && ms >= 0.0 {
                    self.present_us += (ms.min(500.0) * 1e3 - self.present_us) * 0.25;
                    self.update_lead();
                }
            }
            Cmd::Visible(true) => {
                self.visible = true;
                self.presenter.set_visible(true);
                self.dirty = true;
            }
            Cmd::Visible(false) => {
                // Hidden or minimised: pause with silence, and let decoders go.
                self.cancel_queued_play(StateReason::Hidden, None);
                let at = self.clock_us();
                self.stop_playing(at, StateReason::Hidden);
                self.visible = false;
                self.inflight.clear();
                self.presenter.set_visible(false);
                self.p.backend.release();
            }
            Cmd::Volume(volume) => self.p.backend.set_volume(volume),
            Cmd::Play {
                from_us,
                rate,
                seq,
                reply,
            } => {
                let _ = reply.send(self.on_play(from_us, rate, seq));
            }
            Cmd::Pause { reply } => {
                self.cancel_queued_play(StateReason::Pause, None);
                let at = self.clock_us();
                self.stop_playing(at, StateReason::Pause);
                let _ = reply.send(self.playhead_us);
            }
            Cmd::Attached => {
                // A reattach (Retry) reports a lasting error again instead of waiting silently.
                self.last_error = None;
                // A reloaded page counts its seeks from 0 again: none of them is older than what came before.
                self.seq = 0;
                if self.current.is_some() {
                    self.emit(EngineEvent::Ready {
                        session: self.session,
                    });
                    self.dirty = true;
                }
            }
            Cmd::AttachNative { surface, reply } => {
                let _ = reply.send(self.attach_native(surface));
            }
            Cmd::DetachNative => {
                if self.presenter.kind() == PresenterKind::Native {
                    self.use_socket();
                }
            }
            Cmd::PresentLog(reply) => {
                let _ = reply.send(self.present_log.clone());
            }
            Cmd::Media(map) => {
                // Opening the copies' decoders mid-play would stall a frame: playback keeps its files until it stops.
                if self.playing.is_some() {
                    self.pending_media = Some(map);
                } else {
                    self.pending_media = None;
                    self.apply_media(map);
                }
            }
            Cmd::Editing(item) => {
                // Drawn again at once: the layer leaves (or comes back) on the still now.
                if item != self.editing {
                    self.editing = item;
                    self.dirty = true;
                }
            }
            Cmd::MarkWritten { contrib, seq } => {
                // Drawn again at once: the effect appears on the still now (or its compile starts).
                self.p.backend.mark_written(&contrib, seq);
                self.dirty = true;
            }
            Cmd::Shutdown(_) => {}
        }
    }

    /// What the backend's crash guard has for the app since the last frame, and when an open draw mark
    /// next runs out of time (its 2 s: closed then even if no frame draws).
    pub(super) fn emit_guard_events(&mut self) {
        for event in self.p.backend.take_guard_events() {
            self.emit(event.into_event());
        }
        self.guard_due = self
            .p
            .backend
            .guard_deadline()
            .map(tokio::time::Instant::from_std);
    }
}

/// Resolves at `at`; never without one. Cancel-safe.
async fn guard_due(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// The websocket presenter for this engine's frame server.
fn socket_presenter(frames: &FrameSink, rgba_only: bool) -> Box<dyn FramePresenter> {
    let socket = SocketPresenter::new(frames.clone());
    Box::new(if rgba_only {
        socket.rgba_only()
    } else {
        socket
    })
}

impl<B: PreviewBackend> Actor<B> {
    /// Frames go over the socket from now on (the native presenter is dropped, its slots freed).
    fn use_socket(&mut self) {
        self.presenter = socket_presenter(&self.p.frames, self.p.socket_rgba_only);
        self.present_log = None;
        self.p.backend.detach_native();
        // Frames drawn for the native layer never reach the socket: draw the still again.
        self.inflight.clear();
        self.present_us = 0.0;
        self.update_lead();
        self.dirty = true;
    }

    /// The native presenter failed: the socket takes over at once, and the UI is told so it draws frames again.
    fn fall_back(&mut self) {
        tracing::warn!("native preview failed; frames go over the socket");
        self.use_socket();
        crate::platform::present::uninstall_soon();
        self.emit(EngineEvent::Presenter {
            presenter: Presenter::Socket,
        });
    }
}

/// A media event as the UI hears it.
fn media_event(e: crate::media::MediaEvent) -> EngineEvent {
    use crate::api::{AUDIO_DAMAGED, CodedError, ErrorParam};
    use crate::media::MediaEvent;
    match e {
        MediaEvent::Changed { path, kind } => EngineEvent::Media { path, kind },
        MediaEvent::AudioDamaged { path, at_us } => {
            let mut notice = CodedError::new(AUDIO_DAMAGED);
            notice.params.insert(
                "file".into(),
                ErrorParam::Text(crate::log_name(&path).into_owned()),
            );
            notice
                .params
                .insert("atUs".into(), ErrorParam::Number(at_us as f64));
            EngineEvent::Notice { notice }
        }
    }
}
