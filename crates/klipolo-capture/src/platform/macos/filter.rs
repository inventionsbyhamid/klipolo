//! What a ScreenCaptureKit stream captures, built inside a pipe's `start` only: the shareable
//! content list is the first capture API a take touches, after Record (it can show macOS's monthly screen-recording
//! prompt; preflight never reads it). The recorder's live view (`live_view`) uses the same filters, read
//! only once Screen Recording's preflight says it is allowed.
//! - **A display** ("Whole screen", and a game's "display without Klipolo"): the display without any of Klipolo's
//!   processes (`initWithDisplay:excludingApplications:exceptingWindows:`). The worker is Klipolo's own executable with
//!   the same bundle id as the app, so this process, its parent and every running app with Klipolo's bundle id are left
//!   out, the HUD, the camera bubble and the editor included, and their sound too. So is
//!   Notification Centre: a banner shows on the person's screen, never in the file. The system's alert
//!   panels stay in. What is left out is decided in one place, `common::sck::left_out`, and built here (`left_out_apps`).
//! - **An app or a game**: its windows on one display (`initWithDisplay:includingApplications:exceptingWindows:`, so a
//!   game split across processes with one bundle id comes whole), and only its own sound.
//! - **A window**: the window alone, wherever it moves (`initWithDesktopIndependentWindow:`).
//!
//! cidre has no binding for the two application filters, so they are sent through the Objective-C runtime directly.

use std::ffi::{CStr, c_void};
use std::sync::mpsc::sync_channel;
use std::time::Duration;

use cidre::{arc, cg, ns, sc};
use objc2_foundation::NSBundle;

use crate::backend::Target;
use crate::pipe::PipeError;
use crate::platform::common::sck::{ListedApp, USER_DECLINED, left_out};
use crate::platform::common::stills_service::Answered;

/// How long the content list may take (it waits on the person while macOS's prompt is up).
const CONTENT_WAIT: Duration = Duration::from_secs(30);

/// A target's filter, with what the picture is fitted to.
pub(super) struct Picked {
    pub(super) filter: arc::R<sc::ContentFilter>,
    /// The captured area's own pixels.
    pub(super) pixels: (u32, u32),
    /// The display it is on (a window: the main display), for the display's sleep and the HUD.
    pub(super) display: cg::DirectDisplayId,
    /// A single window's picture is scaled into the output as the window changes size.
    pub(super) window: bool,
}

/// The shareable content now: displays, windows and running apps.
pub(super) fn content() -> Result<arc::R<sc::ShareableContent>, PipeError> {
    content_with(None)
}

/// `content`, the read's `answered` told as its answer comes, even after this stopped waiting for it (the stills' one
/// request in flight).
pub(super) fn content_answering(
    answered: Answered,
) -> Result<arc::R<sc::ShareableContent>, PipeError> {
    content_with(Some(answered))
}

fn content_with(mut answered: Option<Answered>) -> Result<arc::R<sc::ShareableContent>, PipeError> {
    let (tx, rx) = sync_channel(1);
    sc::ShareableContent::current_with_ch(move |content, error| {
        if let Some(answered) = &mut answered {
            answered.answer();
        }
        let answer = match (content, error) {
            (Some(c), _) => Ok(c.retained()),
            (None, Some(e)) if e.code() == USER_DECLINED => Err(PipeError::PermissionDenied),
            (None, Some(e)) => Err(PipeError::Os(format!(
                "the shareable content: {}",
                e.code()
            ))),
            (None, None) => Err(PipeError::Os("the shareable content: no answer".into())),
        };
        let _ = tx.try_send(answer);
    });
    rx.recv_timeout(CONTENT_WAIT)
        .unwrap_or_else(|_| Err(PipeError::Os("the shareable content didn't come".into())))
}

/// The filter for `target` from `content`. A display that isn't connected, an app that isn't running or a window that
/// is gone is `NoDevice`.
pub(super) fn pick(content: &sc::ShareableContent, target: &Target) -> Result<Picked, PipeError> {
    match target {
        Target::Display { id } | Target::DisplayWithoutKlipolo { id } => {
            let display = display_of(content, id)?;
            let filter = with_apps(&display, &left_out_apps(content), false)?;
            Ok(picked(filter, display.display_id(), false))
        }
        Target::App { app_id, display } => {
            let display = display_of(content, display)?;
            let apps = apps_of(content, app_id);
            if apps.is_empty() {
                tracing::warn!("filter: the app isn't running");
                return Err(PipeError::NoDevice);
            }
            let filter = with_apps(&display, &apps, true)?;
            Ok(picked(filter, display.display_id(), false))
        }
        Target::Window { id } => {
            let wanted: u32 = id.parse().map_err(|_| PipeError::NoDevice)?;
            let windows = content.windows();
            let window = (windows.iter())
                .find(|w| w.id() == wanted)
                .ok_or(PipeError::NoDevice)?;
            let filter = sc::ContentFilter::with_desktop_independent_window(window);
            Ok(picked(filter, cg::DirectDisplayId::main(), true))
        }
        Target::Camera { .. } | Target::Portal { .. } => Err(PipeError::Unsupported),
    }
}

/// The main display without Klipolo: the whole computer's sound (`audio(System)`).
pub(super) fn main_display_without_klipolo(
    content: &sc::ShareableContent,
) -> Result<Picked, PipeError> {
    let main = cg::DirectDisplayId::main().0.to_string();
    pick(content, &Target::Display { id: main })
}

fn picked(filter: arc::R<sc::ContentFilter>, display: cg::DirectDisplayId, window: bool) -> Picked {
    let (rect, scale) = (filter.content_rect(), f64::from(filter.point_pixel_scale()));
    let side = |points: f64| ((points * scale).round().max(2.0)) as u32;
    Picked {
        pixels: (side(rect.size.width), side(rect.size.height)),
        filter,
        display,
        window,
    }
}

fn display_of(content: &sc::ShareableContent, id: &str) -> Result<arc::R<sc::Display>, PipeError> {
    let wanted: u32 = id.parse().map_err(|_| PipeError::NoDevice)?;
    let displays = content.displays();
    let found = displays.iter().find(|d| d.display_id().0 == wanted);
    found.map(|d| d.retained()).ok_or(PipeError::NoDevice)
}

/// The running apps whose id (`AppInfo::app_id`: the bundle id, else the executable's path) is `app_id`.
fn apps_of(content: &sc::ShareableContent, app_id: &str) -> arc::R<ns::Array<sc::RunningApp>> {
    let apps = content.apps();
    let wanted: Vec<&sc::RunningApp> = (apps.iter())
        .filter(|a| {
            a.bundle_id().to_string() == app_id
                || super::apps::app_id_of_pid(a.process_id()).as_deref() == Some(app_id)
        })
        .collect();
    ns::Array::from_slice(&wanted)
}

/// The apps a whole display leaves out, from ScreenCaptureKit's list (`common::sck::left_out`): this one, its parent
/// (the app), anything with Klipolo's bundle id, and the notification apps. The log names their processes and whether
/// the camera bubble's window is among theirs (numbers only).
fn left_out_apps(content: &sc::ShareableContent) -> arc::R<ns::Array<sc::RunningApp>> {
    // SAFETY: getppid and getpid always succeed.
    let (own, parent) = (std::process::id() as i32, unsafe { libc::getppid() });
    let bundle = NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|b| b.to_string());
    let apps = content.apps();
    let ids: Vec<(i32, String)> = (apps.iter())
        .map(|a| (a.process_id(), a.bundle_id().to_string()))
        .collect();
    let listed: Vec<ListedApp<'_>> = (ids.iter())
        .map(|(pid, bundle_id)| ListedApp {
            pid: *pid,
            bundle_id,
        })
        .collect();
    let picked = left_out(&listed, own, parent, bundle.as_deref());
    let pids: Vec<i32> = picked.iter().map(|&i| ids[i].0).collect();
    let out: Vec<&sc::RunningApp> = (apps.iter().enumerate())
        .filter(|(i, _)| picked.contains(i))
        .map(|(_, a)| a)
        .collect();
    let bubble = super::bubble::window_id().map(|id| {
        (content.windows().iter())
            .find(|w| w.id() == id)
            .and_then(|w| w.owning_app().map(|a| pids.contains(&a.process_id())))
    });
    tracing::info!(
        left_out = out.len(),
        ?pids,
        own,
        bubble_window = ?super::bubble::window_id(),
        bubble_left_out = ?bubble,
        "filter: Klipolo's processes and the notification apps are left out of the display"
    );
    ns::Array::from_slice(&out)
}

/// `initWithDisplay:includingApplications:exceptingWindows:` (or `excludingApplications:`), with no windows excepted.
fn with_apps(
    display: &sc::Display,
    apps: &ns::Array<sc::RunningApp>,
    including: bool,
) -> Result<arc::R<sc::ContentFilter>, PipeError> {
    let selector: &CStr = if including {
        c"initWithDisplay:includingApplications:exceptingWindows:"
    } else {
        c"initWithDisplay:excludingApplications:exceptingWindows:"
    };
    let none = ns::Array::<sc::Window>::new();
    unsafe extern "C" {
        fn sel_registerName(name: *const std::ffi::c_char) -> *const c_void;
        fn objc_msgSend();
    }
    type Init = unsafe extern "C" fn(
        *mut c_void,
        *const c_void,
        *const c_void,
        *const c_void,
        *const c_void,
    ) -> *mut c_void;
    // SAFETY: the selector is SCContentFilter's (macOS 12.3+), sent to a fresh `alloc` with an SCDisplay and two arrays,
    // as its declaration takes them; `init` consumes the allocation and returns +1, which `arc::R` releases.
    unsafe {
        let send: Init = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let allocated: *mut c_void = std::mem::transmute(sc::ContentFilter::alloc());
        let made = send(
            allocated,
            sel_registerName(selector.as_ptr()),
            std::ptr::from_ref(display).cast(),
            std::ptr::from_ref(apps).cast(),
            std::ptr::from_ref(&*none).cast(),
        );
        if made.is_null() {
            return Err(PipeError::Os("the content filter wasn't made".into()));
        }
        Ok(std::mem::transmute::<*mut c_void, arc::R<sc::ContentFilter>>(made))
    }
}
