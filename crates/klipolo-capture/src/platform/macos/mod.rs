//! The macOS backend. Its tasks fill it in:
//! - what this Mac can record and may record, with no capture and no prompts: `displays`, `devices`
//!   (cameras by AVFoundation's discovery, mics and the output from Core Audio), `apps` (the app picker and game
//!   detection) and `permissions`;
//! - the worker shell (`shell`): an accessory app with the menu-bar item (`item`), the HUD (`hud`), the
//!   Carbon shortcuts while armed (`hotkeys`), the power and sleep notices (`power`), the frontmost app (`focus`) and
//!   the parent watch (`parent`), over notification observers (`observe`), with the application delegate (`delegate`:
//!   the item once AppKit's loop runs, a system quit that stops and saves the take first);
//! - the VideoToolbox encoder pinned by its id (`encode`, over `encoders`' list), our IOSurface pool
//!   (`pool`), the probe on generated frames (`probe`, `frames`) and the read-back's decode (`decode`);
//! - ScreenCaptureKit display, app, game and window capture (`screen`, over `filter`'s content filters and
//!   Cap's vendored delegate), the target's or the computer's sound (`sound`), the mic (`CpalMic`) and the captured
//!   display's sleep;
//! - the camera as its own file and lane (`camera_pipe`, over Cap's vendored AVFoundation session in
//!   `camera`, each buffer through `camera_feed`, its notices in `camera_notices`) and Check camera's preview
//!   (`preview`);
//! - the camera bubble (`bubble`), fed by the take's own camera session;
//! - live view (here): the recorder's view of the chosen screen or window (`live_view`), with the take's own
//!   filter and Check camera's JPEG (`jpeg`);
//! - the cursor as its own layer (`cursor`: the mouse-only monitor, the shapes, the self-check and its
//!   sampled fallback), with the system cursor left out of the frames and put back live (`screen`).
//!
//! Nothing in the listing files opens a device or asks for a permission. ScreenCaptureKit's content list, which shows
//! macOS's monthly screen-recording prompt, is read only by the capture pipes' `start`, after Record, and by
//! the live view's, only once Screen Recording's preflight says it is allowed (the one exception). The one
//! request Klipolo makes itself, Screen Recording's (`screen_permission`), runs only on the person's press of "Allow
//! screen recording".

mod apps;
mod bubble;
mod camera;
mod camera_feed;
mod camera_notices;
mod camera_pipe;
mod cursor;
mod decode;
mod delegate;
mod devices;
mod displays;
mod encode;
mod encode_thread;
mod encoders;
mod filter;
mod focus;
mod frames;
mod handoff;
mod host_time;
mod hotkeys;
mod hud;
mod item;
mod jpeg;
mod live_view;
mod observe;
mod own_windows;
mod parent;
mod permissions;
mod pool;
mod power;
mod preview;
mod probe;
mod screen;
mod screen_permission;
mod shell;
mod shell_loop;
mod sound;

pub use crate::platform::common::camera::{FormatFacts, pick_format};
pub use crate::platform::common::sck::{interleave, pipe_event_for, queue_depth};
pub use crate::platform::common::vt_ids::{dedicated_on_mac, engine_on_mac};
pub use camera_pipe::MacCameraPipe;
pub use cursor::MacCursorSource;
pub use decode::decodes;
pub use devices::{input_latency, input_rates};
pub use encode::{EncoderSpec, VtEncoder};
pub use encoders::{EncoderInfo, hardware_encoders};
pub use frames::generated_nv12;
pub use live_view::MacLiveView;
pub use pool::SurfacePool;
pub use preview::MacPreviewPipe;
pub use screen::{MacScreenPipe, frame_interval};
pub use sound::MacSoundPipe;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::backend::{
    AppAudioApi, AudioSpec, Backend, CaptureCaps, DeviceInfo, Devices, HotkeyApi, IndicatorPlace,
    Permissions, PickerKind, PreviewPipe, ProbeReport, ScreenApi, Target, VideoPipes, VideoSpec,
};
use crate::cursor::CursorSource;
use crate::media::StreamInfo;
use crate::mic::CpalMic;
use crate::pipe::{AudioPipe, PipeError};
use crate::shell::Shell;

/// The oldest macOS Klipolo records on: the app's own minimum (`minimumSystemVersion` 15.0, so cidre's `macos_15_0` is
/// safe), and what preflight's `capture.preflight.osTooOld` names.
const OLDEST_MAJOR: u32 = 15;

/// Making it opens nothing (`platform::backend_for_probe` makes it without the consent guard).
pub(super) struct OsBackend {
    /// The display the take's screen pipe records (`CGDirectDisplayID`, 0 before one started): its sleep is the stall
    /// rule's and the heal policy's `display_awake`.
    display: Arc<AtomicU32>,
    /// The camera bubble's live layer, from the camera's pipe to the shell.
    bubble: Arc<bubble::BubbleFeed>,
}

impl OsBackend {
    pub(super) fn new() -> Self {
        Self {
            display: Arc::default(),
            bubble: Arc::default(),
        }
    }
}

impl Backend for OsBackend {
    fn caps(&self) -> CaptureCaps {
        CaptureCaps {
            screen: ScreenApi::ScreenCaptureKit,
            app_audio: AppAudioApi::ScreenCaptureKit,
            hotkeys: HotkeyApi::Carbon,
            focus_events: true,
            picker: PickerKind::OurList,
            hud: true,
            os_supported: records_on(&klipolo_platform::caps::current().os.version),
            indicator: IndicatorPlace::MenuBar,
            // ScreenCaptureKit leaves it out of the frames (and puts it back live); the mouse-only monitor gives
            // exact clicks (the take's self-check samples them where it doesn't), `currentSystem` the shapes.
            cursor: crate::backend::CursorCaps {
                hide: true,
                positions: true,
                shapes: true,
                clicks: klipolo_cursor::ClickSource::Events,
            },
        }
    }

    fn devices(&self) -> Devices {
        Devices {
            displays: displays::list(),
            apps: apps::running(),
            cameras: devices::cameras(),
            mics: devices::mics(),
            output: devices::output(),
        }
    }

    /// Core Audio's input devices alone: a take on a fallback mic asks this once a second.
    fn mics(&self) -> Vec<DeviceInfo> {
        devices::mics()
    }

    fn permissions(&self) -> Permissions {
        permissions::read()
    }

    /// A display, an app or game, or a window, with its sound when asked for; or the camera, its own file. Opens
    /// nothing.
    fn video(&self, spec: &VideoSpec) -> Result<VideoPipes, PipeError> {
        match spec.target {
            Target::Camera { .. } => {
                return Ok(VideoPipes {
                    video: Box::new(MacCameraPipe::new(spec, self.bubble.clone())?),
                    sound: None,
                });
            }
            Target::Portal { .. } => return Err(PipeError::Unsupported),
            _ => {}
        }
        let link = Arc::default();
        let video = MacScreenPipe::new(spec, Arc::clone(&link), self.display.clone());
        let sound = spec
            .sound
            .then(|| Box::new(MacSoundPipe::sharing(link)) as Box<dyn AudioPipe>);
        Ok(VideoPipes {
            video: Box::new(video),
            sound,
        })
    }

    /// A screen's cursor (any screen target): built here, started only by the take's first screen start.
    fn cursor_source(&self, target: &Target) -> Result<Option<Box<dyn CursorSource>>, PipeError> {
        match target {
            Target::Camera { .. } | Target::Portal { .. } => Ok(None),
            _ => {
                let clicks = self.caps().cursor.clicks;
                Ok(Some(Box::new(MacCursorSource::new(target, clicks)?)))
            }
        }
    }

    /// The mic through cpal (by its Core Audio name, which is its id), or the computer's sound without Klipolo's.
    fn audio(&self, spec: &AudioSpec) -> Result<Box<dyn AudioPipe>, PipeError> {
        Ok(match spec {
            AudioSpec::Mic { device } => Box::new(CpalMic::new(device)?),
            AudioSpec::System => Box::new(MacSoundPipe::computer()),
        })
    }

    /// Check camera: building it opens nothing.
    fn preview(&self, camera: &str) -> Result<Box<dyn PreviewPipe>, PipeError> {
        Ok(Box::new(MacPreviewPipe::new(camera)?))
    }

    /// The recorder's live view: building it opens nothing, and its start reads the preflight first (`live_view`).
    fn live_view(&self, target: &Target) -> Result<Box<dyn PreviewPipe>, PipeError> {
        Ok(Box::new(MacLiveView::new(target)?))
    }

    /// Generated frames only, never capture.
    fn probe(&self, budget: Duration) -> ProbeReport {
        probe::run(budget)
    }

    /// Whether the captured display (else the main one) is awake: `CGDisplayIsAsleep`.
    fn display_awake(&self) -> bool {
        let id = match self.display.load(Ordering::Acquire) {
            0 => objc2_core_graphics::CGMainDisplayID(),
            id => id,
        };
        !objc2_core_graphics::CGDisplayIsAsleep(id)
    }

    fn decodes(&self, info: &StreamInfo, keyframe: &[u8]) -> Option<bool> {
        decode::decodes(info, keyframe)
    }

    /// Screen Recording's request (`screen_permission`), on the person's press alone.
    fn request_screen_permission(&self) {
        screen_permission::request();
    }

    fn shell(&self) -> Box<dyn Shell> {
        Box::new(shell::MacShell {
            bubble: self.bubble.clone(),
        })
    }
}

/// Whether macOS `version` ("15.6.1") records: `OLDEST_MAJOR` or later. A version that can't be read counts as one that
/// does, since the app doesn't launch on an older macOS.
fn records_on(version: &str) -> bool {
    let major = version
        .split('.')
        .next()
        .and_then(|m| m.parse::<u32>().ok());
    major.is_none_or(|m| m >= OLDEST_MAJOR)
}
