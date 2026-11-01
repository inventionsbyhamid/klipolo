//! The fake backend's video pipe: synthetic packets (`packets`) or a fixture's real ones on the backend's shared
//! schedule, through a `PacketGate`, as a real pipe sends them.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, PoisonError};

use klipolo_platform::clock::host_ns;

use super::fixture::FixtureVideo;
use super::packets::{SyntheticVideoSpec, frame_ns, synthetic_info, synthetic_packet};
use super::{Scripted, Worker, wait_until};
use crate::backend::{EncoderPick, PresetId};
use crate::cursor::CursorEvent;
use crate::encoder_pick::start_first;
use crate::media::{EncodedPacket, StreamInfo};
use crate::pipe::{PacketGate, PipeError, PipeEvent, Sent, VideoOut, VideoPipe, VideoStats};
use crate::preflight::cursor::CursorPlan;
use crate::preset::{lighter, preset};

/// The fake backend's video pipe: frames on the backend's shared schedule (frame `n` at `epoch_ns + n / fps`, so
/// flashes line up with the audio pipes' blips), from `live_after_ms` after `start`, through a `PacketGate`.
pub(crate) struct FakeVideoPipe {
    pub(crate) spec: SyntheticVideoSpec,
    pub(crate) epoch_ns: u64,
    pub(crate) live_after_ms: u32,
    pub(crate) events: Vec<(u32, PipeEvent)>,
    pub(crate) start_error: Option<PipeError>,
    /// The spec's cursor plan; Burned once the cursor was asked into the frames (`set_cursor_in_frames`).
    pub(crate) cursor: CursorPlan,
    /// It sends the cursor itself (`VideoOut::cursor`): a screen with no cursor source.
    pub(crate) pipe_cursor: bool,
    /// It shows the cursor live when asked; false: from its next start (`Script::live_switch`).
    pub(crate) live_switch: bool,
    /// Each start's cursor plan, in order (`FakeBackend::cursor_plans`).
    pub(crate) plans: Option<Arc<Mutex<Vec<CursorPlan>>>>,
    /// Real packets instead of synthetic ones (`VideoFeed::Fixture`).
    pub(crate) fixture: Option<Arc<FixtureVideo>>,
    /// Counts each start asked for (`FakeBackend::starts`).
    pub(crate) starts: Option<Arc<AtomicU64>>,
    /// The spec's preset (`FakeBackend::video`); `lower_preset` steps it down and takes its size and rate.
    pub(crate) preset: Option<PresetId>,
    /// The spec's encoders, each with whether the script refuses it (`Script::refused_encoders`).
    pub(crate) encoders: Vec<(EncoderPick, bool)>,
    shared: Arc<Shared>,
    worker: Option<Worker>,
}

/// VideoToolbox's −12900, a refused encoder (`kVTCouldNotFindVideoEncoderErr` and friends answer the same way).
const REFUSED: i32 = -12_900;

#[derive(Default)]
struct Shared {
    paused: AtomicBool,
    want_key: AtomicBool,
    delivered: AtomicU64,
    dropped_channel: AtomicU64,
}

impl FakeVideoPipe {
    pub(crate) fn new(spec: SyntheticVideoSpec, epoch_ns: u64) -> Self {
        Self {
            spec,
            epoch_ns,
            live_after_ms: 0,
            events: vec![],
            start_error: None,
            cursor: CursorPlan::None,
            pipe_cursor: false,
            live_switch: true,
            plans: None,
            fixture: None,
            starts: None,
            preset: None,
            encoders: vec![],
            shared: Arc::default(),
            worker: None,
        }
    }
}

impl VideoPipe for FakeVideoPipe {
    fn start(&mut self, out: VideoOut) -> Result<StreamInfo, PipeError> {
        if let Some(starts) = &self.starts {
            starts.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(e) = &self.start_error {
            return Err(e.clone());
        }
        if self.worker.is_some() {
            return Err(PipeError::Busy);
        }
        if let Some(plans) = &self.plans {
            plans
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(self.cursor);
        }
        let picks: Vec<EncoderPick> = self.encoders.iter().map(|(p, _)| p.clone()).collect();
        let refused = |p: &EncoderPick| self.encoders.iter().any(|(q, no)| *no && q == p);
        start_first(&picks, &out.events, |pick| match pick {
            Some(p) if refused(p) => Err(PipeError::Encoder(REFUSED)),
            _ => Ok(()),
        })?;
        let (spec, epoch, shared) = (self.spec.clone(), self.epoch_ns, self.shared.clone());
        let start = host_ns();
        let live_at = start + u64::from(self.live_after_ms) * 1_000_000;
        let mut scripted = Scripted::new(start, std::mem::take(&mut self.events));
        let mut cursor = out.cursor.filter(|_| self.pipe_cursor).map(PipeCursor::new);
        let fixture = self.fixture.clone();
        self.worker = Some(Worker::spawn("fake-video", move |stop| {
            let fps = u64::from(spec.fps.max(1));
            let gop = spec.gop_frames.max(1);
            // The first frame on the shared schedule at or after `live_at`.
            let mut n = (u128::from(live_at.saturating_sub(epoch)) * u128::from(fps))
                .div_ceil(1_000_000_000) as u64;
            let mut gate = PacketGate::new(out.packets);
            let mut since_key: Option<u32> = None;
            loop {
                let frame_at = frame_ns(&spec, n, epoch);
                if !wait_until(frame_at.min(scripted.next_ns()), stop) {
                    return;
                }
                let now = host_ns();
                if scripted.deliver(now, &out.events) {
                    since_key = None;
                }
                if frame_at > now {
                    continue;
                }
                if scripted.flowing {
                    shared.delivered.fetch_add(1, Ordering::Relaxed);
                    if shared.paused.load(Ordering::Relaxed) {
                        since_key = None;
                    } else {
                        let key = shared.want_key.swap(false, Ordering::Relaxed)
                            || since_key.is_none_or(|k| k >= gop);
                        let packet = match &fixture {
                            // A keyframe is the fixture's first frame; the frames after it follow it in order.
                            Some(f) => {
                                let at = if key {
                                    0
                                } else {
                                    since_key.unwrap_or(0) as usize
                                };
                                let (_, data) = &f.frames[at % f.frames.len()];
                                EncodedPacket {
                                    host_ns: frame_at,
                                    keyframe: key,
                                    data: data.clone(),
                                }
                            }
                            None => synthetic_packet(&spec, n, key, frame_at),
                        };
                        match gate.send(packet) {
                            // The encoder made the frame whether or not it fit, so its GOP goes on.
                            Sent::Sent | Sent::Dropped => {
                                since_key = Some(if key { 1 } else { since_key.unwrap_or(0) + 1 })
                            }
                            // A keyframe only when the gate asks, at most every 2 s (`KEYFRAME_ASK_NS`), as a real
                            // encoder is asked.
                            Sent::WantKeyframe => since_key = None,
                            // The session is gone.
                            Sent::Closed => return,
                        }
                        shared
                            .dropped_channel
                            .store(gate.dropped(), Ordering::Relaxed);
                        if let Some(c) = &mut cursor {
                            c.frame(n, fps, frame_at, (spec.width, spec.height));
                        }
                    }
                }
                n += 1;
            }
        }));
        Ok(match &self.fixture {
            Some(f) => f.info.clone(),
            None => synthetic_info(&self.spec),
        })
    }

    fn set_paused(&mut self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    fn request_keyframe(&mut self) {
        self.shared.want_key.store(true, Ordering::Relaxed);
    }

    /// The next start is at the lighter preset's own size and rate (a fixture keeps its own picture).
    fn lower_preset(&mut self) -> bool {
        let Some(next) = self.preset.and_then(lighter) else {
            return false;
        };
        let p = preset(next, self.spec.codec);
        (self.spec.width, self.spec.height) = (p.width, p.height);
        (self.spec.fps, self.spec.gop_frames) = (p.fps, p.fps);
        self.preset = Some(next);
        true
    }

    fn stop(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.stop();
        }
    }

    /// Live when the script allows it and the pipe runs; otherwise from the next start (the caller restarts it).
    fn set_cursor_in_frames(&mut self, on: bool) -> Result<(), PipeError> {
        if on {
            self.cursor = CursorPlan::Burned;
        }
        if self.live_switch && self.worker.is_some() {
            Ok(())
        } else {
            Err(PipeError::Unsupported)
        }
    }

    fn stats(&self) -> VideoStats {
        let delivered = self.shared.delivered.load(Ordering::Relaxed);
        VideoStats {
            delivered,
            complete: delivered,
            dropped_channel: self.shared.dropped_channel.load(Ordering::Relaxed),
            ..VideoStats::default()
        }
    }
}

/// A pipe-sourced cursor (portal metadata, DDA): the area once, a position each frame on a circle around the middle
/// every 4 s, the primary button down for one frame every 2 s, and a beat when the place hasn't changed for 500 ms.
struct PipeCursor {
    tx: SyncSender<CursorEvent>,
    area_sent: bool,
    pressed: bool,
    last: Option<(f32, f32)>,
    last_ns: u64,
}

impl PipeCursor {
    fn new(tx: SyncSender<CursorEvent>) -> Self {
        Self {
            tx,
            area_sent: false,
            pressed: false,
            last: None,
            last_ns: 0,
        }
    }

    fn frame(&mut self, frame: u64, fps: u64, host_ns: u64, (w, h): (u32, u32)) {
        let send = |ev| {
            let _ = self.tx.try_send(ev);
        };
        if !self.area_sent {
            self.area_sent = true;
            send(CursorEvent::Area {
                host_ns,
                w_pts: w as f32,
                h_pts: h as f32,
            });
        }
        let turn = (frame % (fps * 4)) as f32 / (fps * 4) as f32 * std::f32::consts::TAU;
        let (x, y) = (0.5 + 0.3 * turn.cos(), 0.5 + 0.3 * turn.sin());
        if self.last != Some((x, y)) {
            send(CursorEvent::Move { host_ns, x, y });
            (self.last, self.last_ns) = (Some((x, y)), host_ns);
        } else if host_ns >= self.last_ns + 500_000_000 {
            send(CursorEvent::Beat { host_ns, x, y });
            self.last_ns = host_ns;
        }
        let pressed = frame.is_multiple_of(fps * 2);
        if pressed != self.pressed {
            self.pressed = pressed;
            send(CursorEvent::Button {
                host_ns,
                btn: 0,
                down: pressed,
                x,
                y,
            });
        }
    }
}
