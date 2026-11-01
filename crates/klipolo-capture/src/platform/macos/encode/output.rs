//! The encoder's output: VideoToolbox's callback, on VideoToolbox's own thread, one call per frame in order. Each sample
//! is copied out once as an `EncodedPacket` (AVCC already, one allocation: the packet owns its bytes) and sent through
//! the `PacketGate`: a full channel drops it, counted, and the encoder's next frame is forced to a
//! keyframe so no P-frame refers to a lost one. The first sample's format description gives the run's `avcC`/`hvcC`;
//! one that changes mid-run fails the encoder (`PipeEvent::EncoderFailed`, so the heal policy rebuilds it as a new
//! run), since the run's head can't describe it. A failed frame's status is sent once.
//!
//! Per frame: one uncontended lock, one copy of the sample's bytes, a `try_send`; the latency ring is a fixed array
//! (`recent::Recent`, the writer's tick ring too).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Mutex, PoisonError};

use cidre::{cm, os, vt};
use klipolo_platform::clock::host_ns;

use crate::media::{Codec, EncodedPacket, StreamInfo};
use crate::pipe::{PacketGate, PipeEvent, Sent};
use crate::platform::macos::host_time::host_ns_of;
use crate::recent::Recent;

/// Encode latencies kept for the percentiles (`VtEncoder::latency_p99_ms`).
const LATENCY_RING: usize = 1_000;
/// VideoToolbox's "format description change not supported": the stream's description changed mid-run.
pub(super) const CONFIG_CHANGED: i32 = -12_916;

/// What the encoder and its callback share.
pub(super) struct Shared {
    codec: Codec,
    out: Mutex<Out>,
    /// The gate dropped a packet: the next frame is encoded as a keyframe.
    pub(super) force_key: AtomicBool,
    /// Frames VideoToolbox dropped or failed (`VideoStats::dropped_encoder`).
    pub(super) dropped_encoder: AtomicU64,
    /// The session's receiver is gone: the pipe stops.
    pub(super) closed: AtomicBool,
    /// The last keyframe's presentation time (host ns), 0 before one: the screen pipe forces one a second by picture
    /// time, since VideoToolbox's `MaxKeyFrameIntervalDuration` counts frames on a still screen (seen in a smoke run).
    pub(super) last_key_ns: AtomicU64,
    failed: AtomicBool,
    events: SyncSender<PipeEvent>,
}

struct Out {
    gate: PacketGate,
    info: Option<StreamInfo>,
    /// The format description `info` came from, by address: VideoToolbox keeps one while it doesn't change.
    desc: usize,
    /// The last `LATENCY_RING` frames' latencies, in ms.
    latency: Recent<f32, LATENCY_RING>,
    /// Outputs, and the host times of the first and the last (the probe's achieved rate).
    outputs: u64,
    first_ns: Option<u64>,
    last_ns: u64,
}

impl Shared {
    pub(super) fn new(
        codec: Codec,
        out: SyncSender<EncodedPacket>,
        events: SyncSender<PipeEvent>,
    ) -> Self {
        Self {
            codec,
            out: Mutex::new(Out {
                gate: PacketGate::new(out),
                info: None,
                desc: 0,
                latency: Recent::default(),
                outputs: 0,
                first_ns: None,
                last_ns: 0,
            }),
            force_key: AtomicBool::new(false),
            dropped_encoder: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            last_key_ns: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            events,
        }
    }

    fn out(&self) -> std::sync::MutexGuard<'_, Out> {
        self.out.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn info(&self) -> Option<StreamInfo> {
        self.out().info.clone()
    }

    pub(super) fn latency_ms(&self, q: f32) -> f32 {
        self.out().latency.quantile(q)
    }

    pub(super) fn dropped_channel(&self) -> u64 {
        self.out().gate.dropped()
    }

    /// (outputs, first output's host ns, last output's host ns) so far.
    pub(super) fn meter(&self) -> (u64, Option<u64>, u64) {
        let out = self.out();
        (out.outputs, out.first_ns, out.last_ns)
    }

    /// The encoder failed: told once, as the heal policy's `EncoderFailed`.
    pub(super) fn fail(&self, code: i32) {
        if !self.failed.swap(true, Ordering::AcqRel) {
            tracing::warn!(code, "encode: the encoder failed");
            let _ = self.events.try_send(PipeEvent::EncoderFailed { code });
        }
    }

    fn output(
        &self,
        submitted_ns: u64,
        status: os::Status,
        flags: vt::EncodeInfoFlags,
        sample: Option<&cm::SampleBuf>,
    ) {
        if status.0 != 0 {
            self.dropped_encoder.fetch_add(1, Ordering::Relaxed);
            return self.fail(status.0);
        }
        let Some(sample) = sample.filter(|_| !flags.contains(vt::EncodeInfoFlags::FRAME_DROPPED))
        else {
            self.dropped_encoder.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let now = host_ns();
        let mut out = self.out();
        out.latency
            .push(now.saturating_sub(submitted_ns) as f32 / 1e6);
        (out.outputs, out.last_ns) = (out.outputs + 1, now);
        out.first_ns.get_or_insert(now);
        if !self.described(&mut out, sample) {
            drop(out);
            self.dropped_encoder.fetch_add(1, Ordering::Relaxed);
            return self.fail(CONFIG_CHANGED);
        }
        let Some(data) = bytes_of(sample) else {
            self.dropped_encoder.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let packet = EncodedPacket {
            host_ns: host_ns_of(sample.pts()),
            keyframe: sample.is_key_frame(),
            data,
        };
        if packet.keyframe {
            self.last_key_ns.fetch_max(packet.host_ns, Ordering::AcqRel);
        }
        match out.gate.send(packet) {
            Sent::WantKeyframe => self.force_key.store(true, Ordering::Release),
            Sent::Closed => self.closed.store(true, Ordering::Release),
            Sent::Sent | Sent::Dropped => {}
        }
    }

    /// The run's description from its first sample; false when a later one's differs.
    fn described(&self, out: &mut Out, sample: &cm::SampleBuf) -> bool {
        let Some(desc) = sample.format_desc() else {
            return out.info.is_some();
        };
        let at = desc as *const cm::FormatDesc as usize;
        if at == out.desc {
            return true;
        }
        let config = match self.codec {
            Codec::H264 => desc.avcc(),
            Codec::Hevc => desc.hvcc(),
        };
        let Some(config) = config else {
            return false;
        };
        match &out.info {
            Some(info) if info.config != config => false,
            Some(_) => {
                out.desc = at;
                true
            }
            None => {
                let dims = desc.dims();
                out.info = Some(StreamInfo {
                    codec: self.codec,
                    config,
                    width: dims.width.max(0) as u32,
                    height: dims.height.max(0) as u32,
                });
                out.desc = at;
                true
            }
        }
    }
}

/// VideoToolbox's output callback. `shared` is the encoder's (alive until its session is invalidated); `submitted` is
/// the host time `encode` was called at.
pub(super) extern "C" fn output(
    shared: *mut Shared,
    submitted: *mut c_void,
    status: os::Status,
    flags: vt::EncodeInfoFlags,
    sample: Option<&cm::SampleBuf>,
) {
    // SAFETY: the session is made with the encoder's `Shared` and invalidated before it is freed.
    let shared = unsafe { &*shared };
    shared.output(submitted as u64, status, flags, sample);
}

/// The sample's bytes, copied out (a block buffer may be in pieces).
fn bytes_of(sample: &cm::SampleBuf) -> Option<Vec<u8>> {
    let block = sample.data_buf()?;
    let len = block.data_len();
    let mut data = Vec::with_capacity(len);
    while data.len() < len {
        let (piece, _) = block.data_ptr_at(data.len()).ok()?;
        if piece.is_empty() {
            return None;
        }
        let take = piece.len().min(len - data.len());
        data.extend_from_slice(&piece[..take]);
    }
    Some(data)
}
