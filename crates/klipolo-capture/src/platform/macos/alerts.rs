//! Whether one of macOS's own questions is on the screen now (`OsBackend::system_alert`): the on-screen windows from
//! `CGWindowListCopyWindowInfo`, and for those above the normal layer, each owner's executable from `proc_pidpath`
//! (once per process for the take's life), judged by `common::alerts`. No capture and no prompt: a window's layer,
//! bounds, alpha and owner need no permission. A take reads it from its start until its gate, ten times a second, on
//! its core thread. The log says which windows were above the normal layer at the first read and whenever they changed,
//! and the owner of each alert that came (`AlertLog`), so a Mac run shows which process drew one.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, PoisonError};

use crate::platform::common::alerts::{AlertLog, ListedWindow, above_normal, is_alert};

/// Core Graphics' rect: origin and size, f64 each (`CGRect`'s layout). Only its size is read here.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    #[allow(dead_code)]
    x: f64,
    #[allow(dead_code)]
    y: f64,
    w: f64,
    h: f64,
}

/// `kCGWindowListOptionOnScreenOnly`, `kCGWindowListExcludeDesktopElements`.
const ON_SCREEN: u32 = 1 << 0;
const NO_DESKTOP: u32 = 1 << 4;
/// `kCFNumberSInt64Type` and `kCFNumberFloat64Type`.
const SINT64: isize = 4;
const FLOAT64: isize = 6;
/// `PROC_PIDPATHINFO_MAXSIZE`: the longest executable path `proc_pidpath` gives.
const PATH_MAX: u32 = 4096;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGWindowBounds: *const c_void;
    static kCGWindowOwnerPID: *const c_void;
    static kCGWindowAlpha: *const c_void;
    static kCGWindowLayer: *const c_void;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const c_void;
    fn CGRectMakeWithDictionaryRepresentation(dict: *const c_void, rect: *mut Rect) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, at: isize) -> *const c_void;
    fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFNumberGetValue(number: *const c_void, kind: isize, out: *mut c_void) -> bool;
    fn CFRelease(object: *const c_void);
}

/// The take's reads of the system's alerts: its owners' paths so far, and what its log has said.
#[derive(Default)]
pub(super) struct AlertWatch(Mutex<Watch>);

#[derive(Default)]
struct Watch {
    /// Each process's executable, read once (None: it couldn't be).
    paths: HashMap<i32, Option<String>>,
    log: AlertLog,
}

/// One window above the normal layer, read out of the list before it is released.
struct Above {
    pid: i32,
    layer: i64,
    size: (f64, f64),
    alpha: f64,
}

impl AlertWatch {
    /// Whether a system alert is on the screen now (`common::alerts::is_alert`), logging as the module doc says.
    pub(super) fn up(&self) -> bool {
        let mut watch = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let Watch { paths, log } = &mut *watch;
        // SAFETY: getpid and getppid always succeed.
        let ours = [std::process::id() as i32, unsafe { libc::getppid() }];
        let above: Vec<Above> = above_normal_windows()
            .into_iter()
            .filter(|w| !ours.contains(&w.pid))
            .collect();
        for w in &above {
            paths.entry(w.pid).or_insert_with(|| exe_of(w.pid));
        }
        let listed: Vec<ListedWindow<'_>> = (above.iter())
            .map(|w| ListedWindow {
                owner: paths.get(&w.pid).and_then(Option::as_deref),
                pid: w.pid,
                layer: w.layer,
                size: w.size,
                alpha: w.alpha,
            })
            .collect();
        if let Some(windows) = log.windows(&listed) {
            tracing::info!(%windows, "alerts: the windows above the normal layer");
        }
        let alert = listed.iter().find(|w| is_alert(w));
        if let Some(owner) = log.alert(alert) {
            tracing::info!(%owner, "alerts: a system alert is on the screen");
        }
        alert.is_some()
    }
}

/// The on-screen windows above the normal layer (`common::alerts::above_normal`): only these are looked at further.
fn above_normal_windows() -> Vec<Above> {
    let mut above = Vec::new();
    // SAFETY: the copy is +1 (released below) or null; its items are dictionaries it owns while it lives, and everything
    // kept is copied out of them before it goes.
    unsafe {
        let list = CGWindowListCopyWindowInfo(ON_SCREEN | NO_DESKTOP, 0);
        if list.is_null() {
            return above;
        }
        for i in 0..CFArrayGetCount(list) {
            let info = CFArrayGetValueAtIndex(list, i);
            if info.is_null() {
                continue;
            }
            let layer = sint64(info, kCGWindowLayer).unwrap_or(0);
            if !above_normal(layer) {
                continue;
            }
            let Some(pid) = sint64(info, kCGWindowOwnerPID).and_then(|p| i32::try_from(p).ok())
            else {
                continue;
            };
            above.push(Above {
                pid,
                layer,
                size: size(info),
                alpha: float64(info, kCGWindowAlpha).unwrap_or(1.0),
            });
        }
        CFRelease(list);
    }
    above
}

/// The executable of process `pid`, when it can be read.
fn exe_of(pid: i32) -> Option<String> {
    let mut path = vec![0u8; PATH_MAX as usize];
    // SAFETY: the buffer holds `PATH_MAX` bytes, as the call is told; it writes at most that and gives the length.
    let n = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), PATH_MAX) };
    path.truncate(usize::try_from(n).ok().filter(|&n| n > 0)?);
    String::from_utf8(path).ok()
}

/// A window info's size in points (`kCGWindowBounds`); none when it has no bounds.
fn size(info: *const c_void) -> (f64, f64) {
    let mut r = Rect::default();
    // SAFETY: `info` is a live window info dictionary; the bounds value is a dictionary (or null, refused below).
    let ok = unsafe {
        let value = CFDictionaryGetValue(info, kCGWindowBounds);
        !value.is_null() && CGRectMakeWithDictionaryRepresentation(value, &mut r)
    };
    if ok { (r.w, r.h) } else { (0.0, 0.0) }
}

/// A window info's whole number under `key` (`kCGWindowOwnerPID`, `kCGWindowLayer`).
fn sint64(info: *const c_void, key: *const c_void) -> Option<i64> {
    let mut n = 0i64;
    // SAFETY: `info` is a live window info dictionary; the key holds a CFNumber, read into an i64 as `SINT64` says.
    let ok = unsafe {
        let value = CFDictionaryGetValue(info, key);
        !value.is_null() && CFNumberGetValue(value, SINT64, (&raw mut n).cast())
    };
    ok.then_some(n)
}

/// A window info's fraction under `key` (`kCGWindowAlpha`).
fn float64(info: *const c_void, key: *const c_void) -> Option<f64> {
    let mut n = 0f64;
    // SAFETY: `info` is a live window info dictionary; the key holds a CFNumber, read into an f64 as `FLOAT64` says.
    let ok = unsafe {
        let value = CFDictionaryGetValue(info, key);
        !value.is_null() && CFNumberGetValue(value, FLOAT64, (&raw mut n).cast())
    };
    ok.then_some(n)
}
