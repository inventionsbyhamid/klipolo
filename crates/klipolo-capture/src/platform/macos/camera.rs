// SPDX-License-Identifier: MIT
// MIT (Cap's `cap-camera-avfoundation`), Copyright (c) 2023-present Cap Software, Inc.
// Vendored from Cap (https://github.com/CapSoftware/Cap) at d327051c175aa092c06ad9fc6684298b1b5fed6b: crates/camera-avfoundation/src/lib.rs
// Also from Cap's MIT `cap-camera` at the same commit, crates/camera/src/macos.rs: `start_capturing_impl`'s session
// set-up, `AVFoundationRecordingHandle`'s teardown, `min_frame_duration_for_rate`'s idea and the session lifecycle lock.
// Klipolo changes:
// - the sample-buffer delegate's callback gets the buffer itself (`Delivered::Frame`) and each dropped late frame
//   (`Delivered::Dropped`, which the pipe counts), with no stream-start `Instant` or offset: the pipe converts each
//   buffer's own time from the session's synchronization clock to host time (`SessionClock`);
// - the delegate class is registered as `KLIPOLO_CAMERA_OUTPUT` (Cap's `OUTPUT_DELEGATE` could meet another copy);
// - Cap's `list_video_devices`, `YCbCrMatrix`, `CallbackData`, `mach_time_to_microseconds` and the CPU-side
//   `ImageBufExt`/`BaseAddrLockGuard` are left out: Klipolo lists cameras in `devices.rs`, and pixels never reach the CPU
//   on the frame path; the device is found by its unique id (`device`), not by model id;
// - the session (from `cap-camera`): one device input and one video data output in one configuration, the active
//   format and its minimum frame duration set with the device locked, the lock held across `startRunning` (Cap's
//   finding: otherwise the session can reset the format), always discarding late frames, on a serial queue at
//   user-interactive QoS with an autorelease pool per item; `stopRunning`, the delegate cleared and the input and output
//   removed at teardown, under Cap's lifecycle lock;
// - the person's Stop (`StopFlag`) is checked right before each step that asks for or opens the camera: the prompt,
//   the device input and `startRunning` (pinned by `tests/consent_scan.rs`);
// - camera access: AVFoundation's prompt is asked for only here, inside the pipes' `start`, after the person's Record
//   or Check camera, and its answer is waited for with the person's Stop looked at;
// - format facts for `platform::common::camera::pick_format`; the minimum frame durations to try are the pipe's,
//   worked out from the ranges' rates (`frame_durations`), so the struct-returning duration getters Cap avoids on Intel
//   at this cidre rev are never read, and the first one macOS accepts is set;
// - the `workspace-hack` dependency and the deallocation test are left out (the class is Cap's, unchanged in that);
// - the take's camera bubble's preview layer (`Want::preview`) joins the session before the format is set and it runs,
//   mirrored as the person chose.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use cidre::av::capture::{VideoDataOutputSampleBufDelegate, VideoDataOutputSampleBufDelegateImpl};
use cidre::{arc, av, blocks, cm, cv, define_obj_type, dispatch, ns, objc};
use tracing::warn;

use super::host_time::host_ns_of;
use crate::pipe::{PipeError, StopFlag};
use crate::platform::common::camera::{BubbleMirroring, FormatFacts};

/// What the camera's output hands the pipe, on the session's queue.
pub(super) enum Delivered<'a> {
    Frame(&'a cm::SampleBuf),
    /// AVFoundation discarded a late frame (`alwaysDiscardsLateVideoFrames`): the frame path was behind.
    Dropped,
}

pub(super) type OutputDelegateCallback = Box<dyn FnMut(Delivered<'_>)>;

pub struct CallbackOutputDelegateInner {
    callback: OutputDelegateCallback,
    dropped_frames: u64,
}

impl CallbackOutputDelegateInner {
    pub(super) fn new(callback: OutputDelegateCallback) -> Self {
        Self {
            callback,
            dropped_frames: 0,
        }
    }
}

define_obj_type!(
    pub CallbackOutputDelegate + VideoDataOutputSampleBufDelegateImpl + OutputDelegateDeallocation,
    CallbackOutputDelegateInner,
    KLIPOLO_CAMERA_OUTPUT
);

trait OutputDelegateDeallocation {
    fn cls_add_methods(cls: &objc::Class<objc::Id>) {
        extern "C" fn dealloc(delegate: &mut CallbackOutputDelegate, selector: &objc::Sel) {
            let object = delegate as *mut CallbackOutputDelegate;
            unsafe {
                let superclass_dealloc: unsafe extern "C" fn(
                    *mut CallbackOutputDelegate,
                    &objc::Sel,
                ) = std::mem::transmute(objc::NS_OBJECT.method_impl(selector));
                std::ptr::drop_in_place(delegate.inner_mut());
                superclass_dealloc(object, selector);
            }
        }

        // The pinned cidre macro drops only the Rust payload. Its later
        // class_addMethod cannot replace this complete NSObject deallocator.
        let added = unsafe {
            objc::class_addMethod(
                cls,
                objc::sel_reg_name(c"dealloc".as_ptr().cast()),
                std::mem::transmute::<
                    extern "C" fn(&mut CallbackOutputDelegate, &objc::Sel),
                    extern "C" fn(),
                >(dealloc),
                c"v@:".as_ptr().cast(),
            )
        };
        assert!(added, "Camera delegate deallocator already registered");
    }

    fn cls_add_protocol(_: &objc::Class<objc::Id>) {}
}

impl OutputDelegateDeallocation for CallbackOutputDelegate {}

impl VideoDataOutputSampleBufDelegate for CallbackOutputDelegate {}

#[objc::add_methods]
impl VideoDataOutputSampleBufDelegateImpl for CallbackOutputDelegate {
    extern "C" fn impl_capture_output_did_output_sample_buf_from_connection(
        &mut self,
        _cmd: Option<&cidre::objc::Sel>,
        _output: &av::CaptureOutput,
        sample_buf: &cm::SampleBuf,
        _connection: &av::CaptureConnection,
    ) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (self.inner_mut().callback)(Delivered::Frame(sample_buf));
        }));

        if result.is_err() {
            warn!("Suppressed panic in AVFoundation output delegate");
        }
    }

    extern "C" fn impl_capture_output_did_drop_sample_buf_from_connection(
        &mut self,
        _cmd: Option<&cidre::objc::Sel>,
        _output: &av::CaptureOutput,
        sample_buf: &cm::SampleBuf,
        _connection: &av::CaptureConnection,
    ) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let inner = self.inner_mut();
            inner.dropped_frames += 1;

            if inner.dropped_frames == 1 || inner.dropped_frames.is_multiple_of(100) {
                let reason = sample_buf
                    .attach(
                        cm::sample_buffer::buf_attach_keys::dropped_frame_reason(),
                        std::ptr::null_mut(),
                    )
                    .map(|value| value.desc().to_string())
                    .unwrap_or_else(|| "unknown".to_string());

                warn!(
                    count = inner.dropped_frames,
                    reason = %reason,
                    "AVFoundation dropped camera sample buffer(s)"
                );
            }
            (inner.callback)(Delivered::Dropped);
        }));

        if result.is_err() {
            warn!("Suppressed panic in AVFoundation drop delegate");
        }
    }
}

/// How long the camera prompt's answer is waited for: the person may take their time.
const ACCESS_WAIT: Duration = Duration::from_secs(120);
/// How often a wait looks at the person's Stop.
const STOP_LOOK: Duration = Duration::from_millis(100);

/// Cap's lesson: one session starts or stops at a time in a process.
static SESSION_LIFECYCLE_LOCK: Mutex<()> = Mutex::new(());

fn lifecycle() -> std::sync::MutexGuard<'static, ()> {
    SESSION_LIFECYCLE_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

fn stopped() -> PipeError {
    PipeError::Os("stopped before the camera opened".into())
}

/// The camera with this unique id (`DeviceInfo::id`), if it is connected: device inspection, nothing opened.
pub(super) fn device(unique_id: &str) -> Option<arc::R<av::CaptureDevice>> {
    let id = ns::String::with_str(unique_id);
    ns::try_catch(|| av::CaptureDevice::with_unique_id(&id))
        .ok()
        .flatten()
        .filter(|d| d.is_connected())
}

/// One of a camera's formats: what `pick_format` reads, and its frame-rate ranges (min, max fps) for `pick_rate`.
#[derive(Clone)]
pub(super) struct Format {
    pub(super) facts: FormatFacts,
    pub(super) ranges: Vec<(f64, f64)>,
}

/// The camera's formats, in AVFoundation's order (device inspection, nothing opened).
pub(super) fn formats(device: &av::CaptureDevice) -> Vec<Format> {
    objc::ar_pool(|| {
        let all = device.formats();
        all.iter()
            .map(|format| {
                let desc = format.format_desc();
                let dims = desc.dims();
                let sub = desc.media_sub_type();
                let ranges: Vec<(f64, f64)> = (format.video_supported_frame_rate_ranges().iter())
                    .map(|r| (r.min_frame_rate(), r.max_frame_rate()))
                    .collect();
                let max_fps = ranges.iter().map(|r| r.1).fold(0.0, f64::max);
                let nv12 = sub == cv::PixelFormat::_420V.0 || sub == cv::PixelFormat::_420F.0;
                Format {
                    facts: FormatFacts {
                        width: dims.width.max(0) as u32,
                        height: dims.height.max(0) as u32,
                        max_fps,
                        nv12,
                    },
                    ranges,
                }
            })
            .collect()
    })
}

/// Camera access for this process. Not asked yet: AVFoundation's prompt (here only, inside a pipe's `start`, after the
/// person's Record or Check camera), its answer waited for while the person hasn't stopped. Denied or restricted:
/// `PermissionDenied`. No answer within 120 s: `PermissionPending`, never a refusal the person didn't give.
pub(super) fn access(stopping: &StopFlag) -> Result<(), PipeError> {
    let media = av::MediaType::video();
    match av::CaptureDevice::authorization_status_for_media_type(media) {
        Ok(av::AuthorizationStatus::Authorized) => return Ok(()),
        Ok(av::AuthorizationStatus::NotDetermined) => {}
        Ok(_) => return Err(PipeError::PermissionDenied),
        Err(_) => {
            return Err(PipeError::Os(
                "the camera's permission couldn't be read".into(),
            ));
        }
    }
    if stopping.is_set() {
        return Err(stopped());
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let mut answer = blocks::SendBlock::new1(move |granted: bool| {
        let _ = tx.try_send(granted);
    });
    av::CaptureDevice::request_access_for_media_type_ch(media, &mut answer)
        .map_err(|_| PipeError::Os("the camera's permission couldn't be asked".into()))?;
    let until = Instant::now() + ACCESS_WAIT;
    loop {
        match rx.recv_timeout(STOP_LOOK) {
            Ok(true) => return Ok(()),
            Ok(false) => return Err(PipeError::PermissionDenied),
            Err(_) if stopping.is_set() => return Err(stopped()),
            Err(_) if Instant::now() >= until => return Err(PipeError::PermissionPending),
            Err(_) => {}
        }
    }
}

/// What a session delivers: its format (an index into `formats`), the minimum frame durations to try as (value,
/// timescale), the first accepted is set (`platform::common::camera::frame_durations`), and the output's pixels.
pub(super) struct Want {
    pub(super) format: usize,
    pub(super) frame_durations: Vec<(i64, i32)>,
    pub(super) pixels: cv::PixelFormat,
    /// The output scaled to this size (Check camera's preview); None: the format's own.
    pub(super) size: Option<(u32, u32)>,
    /// Also the camera bubble's preview layer (`CameraSession::layer`), made before the session runs, mirrored as this
    /// says.
    pub(super) preview: Option<BubbleMirroring>,
}

/// The session's synchronization clock, to put each buffer's time on the host clock.
pub(super) struct SessionClock(Option<arc::R<cm::Clock>>);

// SAFETY: a CMClock may be read from any thread; it is only read.
unsafe impl Send for SessionClock {}
unsafe impl Sync for SessionClock {}

impl SessionClock {
    /// `t`, a buffer's presentation time on the session's clock, as host ns (`CMSyncConvertTime`). Without a clock
    /// the time is taken as the host clock's, as a video-only session's clock is.
    pub(super) fn host_ns(&self, t: cm::Time) -> u64 {
        let Some(clock) = &self.0 else {
            return host_ns_of(t);
        };
        let host = cm::Clock::host_time_clock();
        // SAFETY: a CMClock is a CMClockOrTimebase (the same CF object), as CMSyncConvertTime takes.
        let (from, to) = unsafe {
            (
                &*(&**clock as *const cm::Clock).cast::<cm::ClockOrTimebase>(),
                &*(host as *const cm::Clock).cast::<cm::ClockOrTimebase>(),
            )
        };
        host_ns_of(from.convert_time_to(t, to))
    }
}

/// One running camera session (Cap's `AVFoundationRecordingHandle`).
pub(super) struct CameraSession {
    _delegate: arc::R<CallbackOutputDelegate>,
    session: arc::R<av::capture::Session>,
    output: arc::R<av::CaptureVideoDataOutput>,
    input: arc::R<av::CaptureDeviceInput>,
    device: arc::R<av::CaptureDevice>,
    _queue: arc::R<dispatch::Queue>,
    is_torn_down: bool,
    pub(super) layer: Option<arc::R<av::CaptureVideoPreviewLayer>>,
}

// SAFETY: the session, its input and output may be started, stopped and reconfigured from any thread (Apple: not the
// main thread); the delegate's callback runs only on the session's own serial queue.
unsafe impl Send for CameraSession {}

impl CameraSession {
    /// The camera `device` at `want`, its buffers to `callback` on a serial queue at user-interactive QoS, running.
    /// The person's Stop is checked right before the device input and right before `startRunning`.
    pub(super) fn open(
        mut device: arc::R<av::CaptureDevice>,
        want: &Want,
        callback: OutputDelegateCallback,
        stopping: &StopFlag,
    ) -> Result<Self, PipeError> {
        let all = device.formats();
        let format = (all.iter())
            .nth(want.format)
            .map(|f| f.retained())
            .ok_or(PipeError::Unsupported)?;
        let os = |what: &str| PipeError::Os(format!("the camera's {what}"));
        if stopping.is_set() {
            return Err(stopped());
        }
        let input = av::CaptureDeviceInput::with_device(&device).map_err(|e| {
            warn!(code = e.code(), "camera: no device input");
            if device.is_connected() {
                PipeError::Busy
            } else {
                PipeError::NoDevice
            }
        })?;
        let attrs = dispatch::Attr::make_with_qos_class(
            Some(&dispatch::Attr::serial_with_ar_pool()),
            dispatch::QosClass::USER_INTERACTIVE,
            0,
        );
        let queue =
            dispatch::Queue::with_label_and_attrs(Some(c"klipolo.capture.camera"), Some(&attrs));
        let delegate = CallbackOutputDelegate::with(CallbackOutputDelegateInner::new(callback));
        let mut output = av::CaptureVideoDataOutput::new();
        output
            .set_video_settings(Some(video_settings(want).as_ref()))
            .map_err(|_| os("output settings"))?;
        output.set_always_discard_late_video_frames(true);
        output.set_sample_buf_delegate(Some(delegate.as_ref()), Some(&queue));
        let mut session = av::capture::Session::new();
        let mut added = false;
        session.configure(|s| {
            if s.can_add_input(&input) && s.can_add_output(&output) {
                s.add_input(&input);
                s.add_output(&output);
                added = true;
            }
        });
        if !added {
            return Err(os("session (its input or output was refused)"));
        }
        let layer =
            (want.preview).map(|mirroring| super::bubble::preview_layer(&session, mirroring));
        let mut opened = Self {
            _delegate: delegate,
            session,
            output,
            input,
            device: device.retained(),
            _queue: queue,
            is_torn_down: false,
            layer,
        };
        let lifecycle_guard = lifecycle();
        // The device stays locked while running starts, or starting can overwrite the active format (Cap's finding).
        let mut lock = device.config_lock().map_err(|_| PipeError::Busy)?;
        lock.set_active_format(&format);
        // A refused duration is an Objective-C exception, caught: the next is tried, and with none accepted the camera
        // runs at the format's own rate.
        let set = (want.frame_durations.iter()).find(|&&(value, scale)| {
            lock.set_active_video_min_frame_duration(cm::Time::new(value, scale))
                .is_ok()
        });
        match set {
            Some(duration) => tracing::info!(?duration, "camera: its frame duration"),
            None if !want.frame_durations.is_empty() => {
                warn!(tried = ?want.frame_durations, "camera: no frame duration was accepted; it runs at the format's own rate");
            }
            None => {}
        }
        if stopping.is_set() {
            drop(lock);
            drop(lifecycle_guard);
            opened.teardown();
            return Err(stopped());
        }
        opened.session.start_running();
        drop(lock);
        Ok(opened)
    }

    /// The session's synchronization clock, once it runs.
    pub(super) fn clock(&self) -> SessionClock {
        SessionClock(self.session.sync_clock().map(|c| c.retained()))
    }

    pub(super) fn session(&self) -> &av::capture::Session {
        &self.session
    }

    pub(super) fn device(&self) -> &av::CaptureDevice {
        &self.device
    }

    fn teardown(&mut self) {
        if self.is_torn_down {
            return;
        }

        self.is_torn_down = true;
        let _session_lifecycle_guard = lifecycle();

        self.session.stop_running();
        self.output
            .set_sample_buf_delegate::<CallbackOutputDelegate>(None, None);
        let output = self.output.clone();
        let input = self.input.clone();
        self.session.configure(move |s| {
            s.remove_output(&output);
            s.remove_input(&input);
        });
    }
}

impl Drop for CameraSession {
    fn drop(&mut self) {
        self.teardown();
    }
}

/// The output's pixel buffers: the pixel format asked for, scaled when a size is given.
fn video_settings(want: &Want) -> arc::R<ns::Dictionary<ns::String, ns::Id>> {
    let key = cv::pixel_buffer::keys::pixel_format();
    let format = ns::Number::with_u32(want.pixels.0);
    let Some((w, h)) = want.size else {
        return ns::Dictionary::with_keys_values(&[key.as_ns()], &[format.as_id_ref()]);
    };
    let (w, h) = (ns::Number::with_u32(w), ns::Number::with_u32(h));
    ns::Dictionary::with_keys_values(
        &[
            key.as_ns(),
            cv::pixel_buffer::keys::width().as_ns(),
            cv::pixel_buffer::keys::height().as_ns(),
        ],
        &[format.as_id_ref(), w.as_id_ref(), h.as_id_ref()],
    )
}
