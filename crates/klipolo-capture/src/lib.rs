//! Klipolo's recorder. Each source the person chose (screen, app or game video, camera, mic, game or
//! computer sound) becomes its own file, written by a worker process that survives an editor crash.
//!
//! - **Seams** (this crate's shared types): `pipe` (a `VideoPipe` gives encoded packets, an
//!   `AudioPipe` PCM blocks, both stamped on the host clock), `backend` (devices without capture, permissions without
//!   prompts, the probe), `shell` (the menu-bar or tray item, HUD, hotkeys, power, focus and the parent watch).
//! - **Consent**: capture begins only after the person presses Record or Check camera.
//!   The exceptions are the Record dialog's previews: the live view of the chosen screen or window (`run::live_view`)
//!   and the stills of each screen and app (`run::thumbs`) run only while the recorder is open and in front and screen
//!   recording is already allowed, and never open the camera or the mic or ask for a permission; its mic meter
//!   (`run::meter`) opens only the chosen mic, which the app starts only while the mic is allowed already or on the
//!   person's own press. Every real backend sits behind the `real-capture` feature, which only the app turns on, and
//!   its constructors call `guard::assert_real_capture_allowed` first, which panics in tests (`KLIPOLO_TEST`).
//! - **The cursor** (`cursor`): the cursor's own layer as a screen take records it (format 2 lines, shapes, the
//!   watchdog that burns it in rather than lose it, resume), behind the `CursorSource` seam.
//! - **Synthetic sources** (feature `synthetic`): encoded packets, PCM and a fake backend and shell, so the muxers, the
//!   writer, recovery and the session state machine are tested on any OS without a device.
//! - **Timing** (`clock`, `drift`): one session clock for every source, with paused time left out; audio runs are
//!   placed on it by a robust line through their anchors.
//! - **Files** (`fmp4`, `caf`): each video run is one growing fragmented MP4, salvageable up to its last complete
//!   fragment and soft-remuxed into a regular MP4 at Stop without copying; each sound run is 24-bit PCM in a CAF of
//!   unknown length, readable to its last whole frame after a crash and sized at Stop.
//! - **The take's folder** (`session`, `journal`, `manifest`): the config written before capture, the lock the live
//!   recorder holds, the append-only journal that is the source of truth after a crash, and the manifest derived from
//!   it.
//! - **Annex B** (`annexb`): FFmpeg's hardware encoders' start-code output (Windows, Linux) turned into the AVCC samples
//!   and `avcC`/`hvcC` records our files carry.
//! - **Writing** (`writer`, `io`, `fail`): one thread per take writes every file at once and makes them durable
//!   together once a second, through a queue that never blocks a capture thread and is measured in media time;
//!   failpoints (debug and `synthetic` builds only) prove each failure path.
//! - **Recovery** (`recover`): unfinished takes found and finished from the files alone after a crash or a power cut,
//!   keeping every durable byte and never importing damage.
//! - **Rules** (`gate`, `health`, `heal`, `disk`): pure rules over times passed in, so nothing records "fine" while a
//!   source is silent or dead: the start gate that waits for real samples from every source, the stall, drop and
//!   silent-mic rules, the heal policy per pipe event with its limits, and the disk outlook.
//! - **Defaults** (`preset`, `probe`, `profile`, `games`): the quality presets, what this computer records by default
//!   (one rule over its probe, data rather than `cfg`) with the FFmpeg encoder ladders for Windows and Linux, per-game
//!   profiles, and the rules that suggest (never start) recording a running game.
//! - **Before and after a take** (`preflight`, `import`): the recorder screen's data from the backend without capture
//!   (devices, permissions, each display's choices, each quality's space and its notes), and a finished or recovered
//!   take as format 4's `Recording`, one asset per run, its files left where they are.
//! - **The microphone** (`mic`, `hosttime`): which mic a take records by default, the real-time hand-off from a sound
//!   device's callback, the host-clock stamps (a device clock far from the host's is re-based), and on macOS and
//!   Windows the cpal microphone, which opens its device only when Record starts it (`real-capture`).
//! - **The previews' pace** (`pace`): Check camera's about 15 frames a second and the live view's 10, paced once, before
//!   a JPEG is made; the live view's picture follows the size of the view that shows it (`live_view_size`).
//! - **The recorder** (`run`, `protocol`, `worker`, `readback`): the session state machine (`run::core`, no threads, no
//!   IO, tested in virtual time), going on after a crashed worker (`run::resume`), the threads that run it with a
//!   backend, the writer and the shell (`run::threads`), the JSON-lines protocol with the app, the 10 s read-back, and
//!   `klipolo --record-worker`'s entry point.
//! - **The shells' shared half** (`shell_view`): the status item's title and menu, the person's words for native menus,
//!   the hotkeys registered only while armed, and the take's power activity.
//! - **Words**: Rust sends `capture.*` codes (`english`), never English for the UI; the native menus word
//!   them in the person's language (`shell_view::Words`).

pub mod annexb;
pub mod backend;
pub mod caf;
pub mod clock;
pub mod cursor;
pub mod devices;
pub mod disk;
pub mod drift;
pub mod encoder_pick;
pub mod english;
pub mod fail;
pub mod fmp4;
pub mod games;
pub mod gate;
pub mod guard;
pub mod heal;
pub mod health;
pub mod hosttime;
pub mod import;
pub mod io;
pub mod journal;
pub mod live_view_size;
pub mod manifest;
pub mod media;
pub mod mic;
pub mod pace;
pub mod pipe;
pub mod platform;
pub mod preflight;
pub mod preset;
pub mod preview_wire;
pub mod probe;
pub mod profile;
pub mod protocol;
pub mod readback;
mod recent;
pub mod recover;
pub mod run;
pub mod session;
pub mod shell;
pub mod shell_view;
#[cfg(feature = "synthetic")]
pub mod synthetic;
#[cfg(all(feature = "real-capture", target_os = "macos"))]
mod vendor;
pub mod worker;
pub mod writer;

pub use backend::{
    AppAudioApi, AppInfo, AudioSpec, Backend, CaptureCaps, CursorCaps, DeviceInfo, Devices,
    DisplayInfo, EncoderPick, EncoderResult, HotkeyApi, IndicatorPlace, Permission, Permissions,
    PickerKind, PresetId, PreviewFrame, PreviewPipe, ProbeReport, ScreenApi, Target, Transport,
    VideoPipes, VideoSpec,
};
pub use clock::SessionClock;
pub use drift::{Anchor, Fit};
pub use media::{AudioFormat, Codec, EncodedPacket, PcmBlock, SourceId, StreamInfo};
pub use pipe::{
    AudioOut, AudioPipe, AudioStats, EVENT_CHANNEL, PACKET_CHANNEL, PCM_CHANNEL, PacketGate,
    PipeError, PipeEvent, Sent, StopFlag, VideoOut, VideoPipe, VideoStats,
};
pub use shell::{
    Flash, HotkeyAction, KeyBindings, Level, MenuAction, PowerEvent, Shell, ShellInput, ShellIo,
    ShellOutput, StatusView, WorkerState,
};
