//! The capture seams every OS implements: a `VideoPipe` captures and encodes, an `AudioPipe` captures PCM. Both send
//! through bounded channels and never block a capture callback: a full channel drops and counts.
//!
//! **Starting.** A pipe's `start` may block for as long as a system picker is open (Linux's ScreenCast portal waits for
//! the person's choice), so the session starts its pipes concurrently, each on its own thread, and never holds a lock
//! across `start`. The start gate (`gate::StartGate`) is made once every pipe's `start` has returned, so its 3 s
//! "isn't sending anything yet" counts from then, not from a source's first sample: a source that never sends anything
//! is still asked about.

mod gate;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};

use klipolo_model::coded::CodedError;
use serde::{Deserialize, Serialize};

use crate::cursor::CursorEvent;
use crate::english::msg;
use crate::media::{AudioFormat, EncodedPacket, PcmBlock, StreamInfo};

pub use gate::{KEYFRAME_ASK_NS, PacketGate, Sent};

/// Encoded packets a video pipe may have in flight: 2 s at 60 fps.
pub const PACKET_CHANNEL: usize = 120;
/// PCM blocks an audio pipe may have in flight: 2 s of 10 ms blocks.
pub const PCM_CHANNEL: usize = 200;
/// Pipe events in flight.
pub const EVENT_CHANNEL: usize = 64;

/// Where a video pipe sends.
pub struct VideoOut {
    pub packets: SyncSender<EncodedPacket>,
    /// Screen video whose cursor comes with its frames (portal metadata, DDA), when the cursor is recorded. A pipe that
    /// sends it re-sends its last position as `CursorEvent::Beat` every 500 ms while still (the watchdog's heartbeat).
    pub cursor: Option<SyncSender<CursorEvent>>,
    pub events: SyncSender<PipeEvent>,
    pub stopping: StopFlag,
}

/// Where an audio pipe sends.
pub struct AudioOut {
    pub blocks: SyncSender<PcmBlock>,
    pub events: SyncSender<PipeEvent>,
    pub stopping: StopFlag,
}

/// The person's Stop (or Cancel, Disarm) as the session read it (consent before each device open). A
/// pipe checks it right before it opens its device, so a start that waited on a system prompt or picker opens nothing
/// once the person has stopped. The default is never set.
#[derive(Clone, Debug, Default)]
pub struct StopFlag(Arc<AtomicBool>);

impl StopFlag {
    /// The session's own flag.
    pub fn new(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }

    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Why `first_or_stop` gave nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoFirst {
    /// The person stopped while it waited.
    Stopped,
    TimedOut,
    /// The sender went (the encoder thread ended).
    Gone,
}

/// How often `first_or_stop` looks at the person's Stop.
const STOP_LOOK: Duration = Duration::from_millis(100);

/// The first thing from `rx` within `limit`, looking at the person's Stop every 100 ms: a start that waits for its
/// first frame ends at once when the person stops, so a slow camera's light never stays on for the rest of the wait
/// (the screen's wait too).
pub fn first_or_stop<T>(
    rx: &Receiver<T>,
    limit: Duration,
    stopping: &StopFlag,
) -> Result<T, NoFirst> {
    let until = Instant::now() + limit;
    loop {
        if stopping.is_set() {
            return Err(NoFirst::Stopped);
        }
        let Some(left) = until.checked_duration_since(Instant::now()) else {
            return Err(NoFirst::TimedOut);
        };
        match rx.recv_timeout(left.min(STOP_LOOK)) {
            Ok(first) => return Ok(first),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(NoFirst::Gone),
        }
    }
}

/// Capture plus encode for one video source (a screen, an app, a game or a camera). A packet is sent only for a frame
/// with new pixels (ScreenCaptureKit's `Complete`), never for an idle buffer, and a blank picture (a protected window,
/// a sleeping display) is reported with `PipeEvent::Blank`: the start gate counts packets as real frames except while
/// the picture is blank, so a black screen can't pass it.
pub trait VideoPipe: Send {
    /// Starts capturing and sending. The stream's description comes back once the encoder knows it. It may block while
    /// a system picker is open (the module's "Starting"): call it on its own thread, holding no lock.
    fn start(&mut self, out: VideoOut) -> Result<StreamInfo, PipeError>;
    /// While paused, captured frames are dropped (not counted as drops); the first frame after resuming is a keyframe.
    fn set_paused(&mut self, paused: bool);
    /// The encoder's next frame is a keyframe.
    fn request_keyframe(&mut self);
    /// Steps the pipe down to `preset::lighter` of its preset for its next `start`: the heal policy's `LowerPreset`,
    /// after an encoder that kept failing was rebuilt three times. The session lowers its own idea of
    /// the source's rate by the same rule. False when there is nothing lighter; the restart then keeps the preset.
    fn lower_preset(&mut self) -> bool;
    /// Stops capturing; nothing is sent afterwards. Idempotent.
    fn stop(&mut self);
    fn stats(&self) -> VideoStats;
    /// From now on the frames show the system cursor (the fallback). Ok once the stream confirmed it;
    /// Err: it couldn't switch live, the pipe's next start shows it, and the caller heals with a restart. A pipe whose
    /// backend can hide the cursor (`CursorCaps::hide`) implements it.
    fn set_cursor_in_frames(&mut self, _on: bool) -> Result<(), PipeError> {
        Err(PipeError::Unsupported)
    }
}

/// Capture for one sound source (a mic, a game's or the computer's sound).
pub trait AudioPipe: Send {
    /// Starts capturing and sending. Like `VideoPipe::start`, it may block: call it on its own thread, holding no lock.
    fn start(&mut self, out: AudioOut) -> Result<AudioFormat, PipeError>;
    /// Stops capturing; nothing is sent afterwards. Idempotent.
    fn stop(&mut self);
    fn stats(&self) -> AudioStats;
}

/// A video pipe's counters since it started (the health rules read their deltas once a second).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoStats {
    /// Frames the OS delivered.
    pub delivered: u64,
    /// Delivered frames with new pixels (ScreenCaptureKit's `Complete`).
    pub complete: u64,
    /// Delivered frames with nothing new (`Idle`).
    pub idle: u64,
    /// Delivered frames that were blank (a protected window, a sleeping display).
    pub blank: u64,
    /// Frames dropped because our GPU pool was full.
    pub dropped_pool: u64,
    /// Frames the encoder dropped or failed.
    pub dropped_encoder: u64,
    /// Packets dropped at a full packet channel, including those `PacketGate` held back until a keyframe.
    pub dropped_channel: u64,
    /// Times a quiet stream answered the pipe's nudge without an error (ScreenCaptureKit's `updateConfiguration:`): it
    /// is live with nothing new to send, so the stall rule counts these with `delivered`.
    pub answered: u64,
    pub encode_p99_ms: f32,
    /// Cursor events dropped on the way to the session (its queue was full).
    #[serde(default)]
    pub cursor_dropped: u64,
}

/// An audio pipe's counters since it started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStats {
    pub callbacks: u64,
    pub frames: u64,
    /// Blocks dropped on the way to the session: at a full PCM channel, at a full hand-off ring (`mic::ring`), and the
    /// blocks still held when the pipe stopped.
    pub dropped_channel: u64,
}

/// What a pipe reports besides media. The session's heal policy decides what each means (`heal::HealPolicy`).
#[derive(Clone, Debug, PartialEq)]
pub enum PipeEvent {
    Stalled,
    DeviceLost,
    DeviceBack,
    StoppedBySystem,
    /// The OS's own "Stop sharing" control.
    StoppedByUser,
    PermissionRevoked,
    /// The recorded app or window went away (a game quit).
    TargetGone,
    TargetBack,
    EncoderFailed {
        code: i32,
    },
    /// The picture went blank (true) or came back (false).
    Blank(bool),
    /// A one-time warning: a `capture.*` code (`capture.health.presenterOverlay`).
    Note(String),
    /// The system picker's new restore token, for the next take of the same target (Linux's ScreenCast portal: each
    /// token is single-use and every start returns a fresh one). The session saves it to the game's profile.
    RestoreToken(String),
}

/// Why a pipe couldn't start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipeError {
    Unsupported,
    PermissionDenied,
    /// The system's permission prompt was still waiting for the person's answer when the pipe stopped waiting: not a
    /// refusal.
    PermissionPending,
    NoDevice,
    Busy,
    /// The encoder's own status code.
    Encoder(i32),
    /// The OS's reason, for the log only.
    Os(String),
}

impl PipeError {
    /// The error as a `capture.error.*` code. The encoder's status and the OS's reason ride along for the log; the
    /// messages don't show them.
    pub fn coded(&self) -> CodedError {
        match self {
            Self::Unsupported => msg!("capture.error.unsupported"),
            Self::PermissionDenied => msg!("capture.error.permission"),
            Self::PermissionPending => msg!("capture.error.permissionPending"),
            Self::NoDevice => msg!("capture.error.noDevice"),
            Self::Busy => msg!("capture.error.busy"),
            Self::Encoder(status) => msg!("capture.error.encoder", status = i64::from(*status)),
            Self::Os(detail) => msg!("capture.error.os", detail = detail.as_str()),
        }
    }
}

impl std::fmt::Display for PipeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encoder(status) => {
                write!(f, "{} ({status})", crate::english::english(&self.coded()))
            }
            Self::Os(detail) => write!(f, "{} ({detail})", crate::english::english(&self.coded())),
            _ => f.write_str(&crate::english::english(&self.coded())),
        }
    }
}

impl std::error::Error for PipeError {}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

    use super::*;

    #[test]
    fn a_first_frame_wait_ends_on_the_person_s_stop() {
        let flag = Arc::new(AtomicBool::new(false));
        let stopping = StopFlag::new(flag.clone());
        // Nothing comes, and the person stops after 150 ms: the 5 s wait ends within a look.
        let (_tx, rx) = sync_channel::<u32>(1);
        let stop = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            stop.store(true, Ordering::Release);
        });
        let at = Instant::now();
        assert_eq!(
            first_or_stop(&rx, Duration::from_secs(5), &stopping),
            Err(NoFirst::Stopped)
        );
        assert!(
            at.elapsed() < Duration::from_millis(400),
            "{:?}",
            at.elapsed()
        );
        // A first frame comes; nothing comes in time; the sender goes.
        flag.store(false, Ordering::Release);
        let (tx, rx) = sync_channel(1);
        tx.send(7).unwrap();
        assert_eq!(first_or_stop(&rx, Duration::from_secs(5), &stopping), Ok(7));
        let short = Duration::from_millis(120);
        assert_eq!(first_or_stop(&rx, short, &stopping), Err(NoFirst::TimedOut));
        drop(tx);
        assert_eq!(first_or_stop(&rx, short, &stopping), Err(NoFirst::Gone));
    }
}
