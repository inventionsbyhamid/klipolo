//! Whether one of macOS's own questions is on the screen now (`OsBackend::system_alert`): the on-screen windows from
//! `CGWindowListCopyWindowInfo`, each owner's executable from `proc_pidpath`, judged by `common::alerts`. No capture and
//! no prompt: a window's bounds, alpha and owner need no permission. A take reads it from its start until its gate, ten
//! times a second, on its core thread.

use std::ffi::c_void;

use crate::platform::common::alerts::{ListedWindow, is_alert};

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

/// Whether a system alert is on the screen now (`common::alerts::is_alert`).
pub(super) fn up() -> bool {
    let mut found = false;
    // SAFETY: the copy is +1 (released below) or null; its items are dictionaries it owns while it lives.
    unsafe {
        let list = CGWindowListCopyWindowInfo(ON_SCREEN | NO_DESKTOP, 0);
        if list.is_null() {
            return false;
        }
        for i in 0..CFArrayGetCount(list) {
            let info = CFArrayGetValueAtIndex(list, i);
            if info.is_null() {
                continue;
            }
            let Some(pid) = sint64(info, kCGWindowOwnerPID) else {
                continue;
            };
            let owner = exe_of(pid);
            let window = ListedWindow {
                owner: owner.as_deref(),
                size: size(info),
                alpha: float64(info, kCGWindowAlpha).unwrap_or(1.0),
            };
            if is_alert(&window) {
                found = true;
                break;
            }
        }
        CFRelease(list);
    }
    found
}

/// The executable of process `pid`, when it can be read.
fn exe_of(pid: i64) -> Option<String> {
    let pid = libc::c_int::try_from(pid).ok()?;
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

/// A window info's whole number under `key` (`kCGWindowOwnerPID`).
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
