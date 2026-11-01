//! What AVFoundation says about a recording camera, as the session's pipe events, deduplicated by the pure
//! `platform::common::camera::CameraWatch`:
//! - **The device** (`DeviceNotices`, for the pipe's whole life, since a lost camera's return must reach a pipe whose
//!   start failed): `AVCaptureDeviceWasDisconnectedNotification` for this camera's unique id is `DeviceLost`, and
//!   `WasConnected` for the same id `DeviceBack`, sent to the latest start's events.
//! - **The session** (`SessionNotices`, one per start): `AVCaptureSessionRuntimeErrorNotification` and
//!   `WasInterruptedNotification` for this session are `Stalled` (a loss if the camera is gone), and key-value
//!   observation of the device's `suspended` (a closed lid, a privacy shutter) is `Blank(true/false)`.
//!
//! The observers run on the posting thread (no queue): each only sends an event without waiting. They are removed when
//! dropped. Nothing here opens the camera.

use std::ffi::c_void;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, PoisonError};

use cidre::{arc, av, ns};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_foundation::{NSString, ns_string};

use crate::pipe::PipeEvent;
use crate::platform::common::camera::CameraWatch;

/// What the pipe does with each event too.
type OnEvent = Box<dyn Fn(&PipeEvent) + Send>;

/// One camera's watch and where its events go now, shared by the observers.
#[derive(Default)]
pub(super) struct Shared {
    watch: Mutex<CameraWatch>,
    events: Mutex<Option<SyncSender<PipeEvent>>>,
    /// What the pipe hears too (a suspension counts as blank pictures).
    on: Mutex<Option<OnEvent>>,
}

impl Shared {
    fn send(&self, event: Option<PipeEvent>) {
        let Some(event) = event else { return };
        if let Some(on) = self
            .on
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            on(&event);
        }
        if let Some(tx) = self
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            let _ = tx.try_send(event);
        }
    }

    fn watch(&self) -> std::sync::MutexGuard<'_, CameraWatch> {
        self.watch.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Observers on the default notification center, removed when dropped.
struct Guards {
    _kept: Vec<ns::NotificationGuard>,
}

// SAFETY: NSNotificationCenter may add and remove observers from any thread; the guards are only dropped.
unsafe impl Send for Guards {}

/// The unique id of a notice's device.
fn unique_id_of(note: &ns::Notification) -> Option<String> {
    let object = note.id()?;
    // SAFETY: a device notice's object is the AVCaptureDevice.
    let device = unsafe { &*(&*object as *const ns::Id).cast::<av::CaptureDevice>() };
    Some(device.unique_id().to_string())
}

/// The camera's disconnect and return, by unique id, for the pipe's whole life.
pub(super) struct DeviceNotices {
    shared: Arc<Shared>,
    _guards: Guards,
}

impl DeviceNotices {
    pub(super) fn new(unique_id: &str, on: impl Fn(&PipeEvent) + Send + 'static) -> Self {
        let shared = Arc::new(Shared::default());
        *shared.on.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(on));
        let mut center = ns::NotificationCenter::default();
        let names = av::capture::device_notifications::was_disconnected();
        let (lost, ours) = (shared.clone(), unique_id.to_owned());
        let disconnected = center.add_observer_guard(names, None, None, move |note| {
            let id = unique_id_of(note);
            let event = id.and_then(|id| lost.watch().disconnected(&id, &ours));
            lost.send(event);
        });
        let (back, ours) = (shared.clone(), unique_id.to_owned());
        let names = av::capture::device_notifications::was_connected();
        let connected = center.add_observer_guard(names, None, None, move |note| {
            let id = unique_id_of(note);
            let event = id.and_then(|id| back.watch().connected(&id, &ours));
            back.send(event);
        });
        Self {
            shared,
            _guards: Guards {
                _kept: vec![disconnected, connected],
            },
        }
    }

    /// The latest start's events: a loss or a return goes there, even after a failed start.
    pub(super) fn send_to(&self, events: SyncSender<PipeEvent>) {
        *self
            .shared
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(events);
    }

    /// The session's own notices for one start, through the same watch.
    pub(super) fn session(
        &self,
        session: &av::capture::Session,
        device: &av::CaptureDevice,
    ) -> SessionNotices {
        let mut center = ns::NotificationCenter::default();
        let object = Some(session.as_id_ref());
        let mut guards = vec![];
        for name in [
            av::capture::session::notifications::runtime_error(),
            av::capture::session::notifications::was_interrupted(),
        ] {
            let shared = self.shared.clone();
            let device = Held(device.retained());
            guards.push(center.add_observer_guard(name, object, None, move |note| {
                // The whole `Held`, not its field (a closure would capture that alone).
                let (_, held) = (note, &device);
                tracing::warn!("camera: the session reports a runtime error or an interruption");
                let event = shared.watch().trouble(held.0.is_connected());
                shared.send(event);
            }));
        }
        let shared = self.shared.clone();
        let suspended = Suspended::observe(device, move |suspended| {
            let event = shared.watch().suspended(suspended);
            shared.send(event);
        });
        SessionNotices {
            _guards: Guards { _kept: guards },
            _suspended: suspended,
        }
    }
}

/// A device kept for an observer block.
struct Held(arc::R<av::CaptureDevice>);

// SAFETY: an AVCaptureDevice's properties may be read from any thread.
unsafe impl Send for Held {}
unsafe impl Sync for Held {}

/// One start's session notices; removed when dropped.
pub(super) struct SessionNotices {
    _guards: Guards,
    _suspended: Option<Suspended>,
}

/// Key-value observation of a device's `suspended`, removed when dropped.
struct Suspended {
    device: Retained<AnyObject>,
    watch: Retained<SuspendWatch>,
}

// SAFETY: KVO observers may be added and removed from any thread; both objects are only kept and removed.
unsafe impl Send for Suspended {}

impl Suspended {
    /// Calls `on` with `isSuspended` now and at every change; None when the device doesn't have it or can't be
    /// observed.
    fn observe(
        device: &av::CaptureDevice,
        on: impl Fn(bool) + Send + Sync + 'static,
    ) -> Option<Self> {
        // SAFETY: an AVCaptureDevice is an Objective-C object; this retains the same object as `AnyObject`.
        let device = unsafe {
            Retained::retain(
                (device as *const av::CaptureDevice)
                    .cast::<AnyObject>()
                    .cast_mut(),
            )
        }?;
        // SAFETY: `respondsToSelector:` takes a selector and returns BOOL on every NSObject.
        let has: bool = unsafe { msg_send![&*device, respondsToSelector: sel!(isSuspended)] };
        if !has {
            return None;
        }
        let watch = SuspendWatch::new(Box::new(on));
        // SAFETY: `addObserver:forKeyPath:options:context:` with an NSObject observer that implements
        // `observeValueForKeyPath:ofObject:change:context:`; New | Initial (1 | 4); removed in `drop`.
        let added = ns::try_catch(|| unsafe {
            let _: () = msg_send![&*device, addObserver: &*watch, forKeyPath: key(), options: 5usize, context: std::ptr::null_mut::<c_void>()];
        });
        added.ok().map(|()| Self { device, watch })
    }
}

impl Drop for Suspended {
    fn drop(&mut self) {
        // SAFETY: the observer this added, for the same key path.
        let _ = ns::try_catch(|| unsafe {
            let _: () = msg_send![&*self.device, removeObserver: &*self.watch, forKeyPath: key()];
        });
    }
}

fn key() -> &'static NSString {
    ns_string!("suspended")
}

pub(super) struct WatchIvars {
    on: Box<dyn Fn(bool) + Send + Sync>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `SuspendWatch` doesn't implement `Drop`.
    #[unsafe(super = NSObject)]
    #[name = "KlipoloCameraSuspendWatch"]
    #[ivars = WatchIvars]
    struct SuspendWatch;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for SuspendWatch {}

    impl SuspendWatch {
        /// KVO's change notice: the device's `isSuspended` now.
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            _key: Option<&NSString>,
            object: Option<&AnyObject>,
            _change: Option<&AnyObject>,
            _context: *mut c_void,
        ) {
            let Some(object) = object else { return };
            // SAFETY: the observed object is the AVCaptureDevice, which answers `isSuspended` with a BOOL.
            let suspended: bool = unsafe { msg_send![object, isSuspended] };
            (self.ivars().on)(suspended);
        }
    }
);

impl SuspendWatch {
    fn new(on: Box<dyn Fn(bool) + Send + Sync>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(WatchIvars { on });
        // SAFETY: NSObject's `init` has this signature.
        unsafe { msg_send![super(this), init] }
    }
}
