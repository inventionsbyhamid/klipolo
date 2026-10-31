//! AppKit's class cursors as system shapes: each one's key on this Mac, made once as the sampler
//! starts, so a recorded shape whose key is one of them is named (`sys`). The table has the macOS 15 resize cursors in
//! every direction beside the ones the macOS 15 SDK marks to be deprecated (c9-findings), each asked only when this
//! macOS answers it. C9 found `currentSystem`'s own images can differ from the class cursors': an unmatched shape
//! simply has no `sys`.

use std::ffi::CStr;

use klipolo_cursor::SystemShape;
use objc2::rc::{AutoreleasePool, autoreleasepool};
use objc2::runtime::{AnyObject, Sel};
use objc2::{class, msg_send, sel};

use super::image;

/// AppKit's class cursors that take no argument, and the system shape each is.
const CLASS: &[(&CStr, SystemShape)] = &[
    (c"arrowCursor", SystemShape::Arrow),
    (c"IBeamCursor", SystemShape::IBeam),
    (c"IBeamCursorForVerticalLayout", SystemShape::VerticalIBeam),
    (c"pointingHandCursor", SystemShape::PointingHand),
    (c"openHandCursor", SystemShape::OpenHand),
    (c"closedHandCursor", SystemShape::ClosedHand),
    (c"crosshairCursor", SystemShape::Crosshair),
    (c"operationNotAllowedCursor", SystemShape::NotAllowed),
    (c"dragCopyCursor", SystemShape::DragCopy),
    (c"dragLinkCursor", SystemShape::DragLink),
    (c"contextualMenuCursor", SystemShape::ContextMenu),
    (c"zoomInCursor", SystemShape::ZoomIn),
    (c"zoomOutCursor", SystemShape::ZoomOut),
    (c"columnResizeCursor", SystemShape::ResizeLeftRight),
    (c"rowResizeCursor", SystemShape::ResizeUpDown),
    // To be deprecated (the macOS 15 SDK), while macOS still has them.
    (c"resizeLeftCursor", SystemShape::ResizeLeftRight),
    (c"resizeRightCursor", SystemShape::ResizeLeftRight),
    (c"resizeLeftRightCursor", SystemShape::ResizeLeftRight),
    (c"resizeUpCursor", SystemShape::ResizeUpDown),
    (c"resizeDownCursor", SystemShape::ResizeUpDown),
    (c"resizeUpDownCursor", SystemShape::ResizeUpDown),
];

/// The keys of AppKit's class cursors on this Mac (about 50 looks, once), each with the system shape it is.
pub(super) fn system_table() -> Vec<(u64, SystemShape)> {
    let mut table = Vec::new();
    autoreleasepool(|pool| {
        let mut add = |cursor: Option<&AnyObject>, sys: SystemShape| {
            if let Some(key) = cursor.and_then(|c| image::key(c, image::identity(c)))
                && !table.iter().any(|(k, _)| *k == key)
            {
                table.push((key, sys));
            }
        };
        for (name, sys) in CLASS {
            add(class_cursor(pool, name), *sys);
        }
        // NSHorizontalDirections and NSVerticalDirections: 1, 2, both.
        for d in 1..=3 {
            add(
                resize_cursor(pool, Resize::Column(d)),
                SystemShape::ResizeLeftRight,
            );
            add(
                resize_cursor(pool, Resize::Row(d)),
                SystemShape::ResizeUpDown,
            );
        }
        // NSCursorFrameResizePosition (top 1, left 2, bottom 4, right 8, and the corners) × inward, outward, both.
        for (at, sys) in [
            (1, SystemShape::ResizeUpDown),
            (4, SystemShape::ResizeUpDown),
            (2, SystemShape::ResizeLeftRight),
            (8, SystemShape::ResizeLeftRight),
            (3, SystemShape::ResizeNwse),
            (12, SystemShape::ResizeNwse),
            (9, SystemShape::ResizeNesw),
            (6, SystemShape::ResizeNesw),
        ] {
            for d in 1..=3 {
                add(resize_cursor(pool, Resize::Frame(at, d)), sys);
            }
        }
    });
    table
}

/// The class cursor `name` (`arrowCursor`), when this macOS has it.
pub(super) fn class_cursor<'p>(_pool: AutoreleasePool<'p>, name: &CStr) -> Option<&'p AnyObject> {
    let sel = Sel::register(name);
    // SAFETY: a class method taking nothing and returning a cursor, asked only once NSCursor says it answers it.
    unsafe {
        let responds: bool = msg_send![class!(NSCursor), respondsToSelector: sel];
        if !responds {
            return None;
        }
        let cursor: *mut AnyObject = msg_send![class!(NSCursor), performSelector: sel];
        cursor.as_ref()
    }
}

/// The macOS 15 resize cursors (`columnResizeCursorInDirections:`, `rowResizeCursorInDirections:`,
/// `frameResizeCursorFromPosition:inDirections:`), by their selector's arguments, when this macOS has them.
pub(super) fn resize_cursor<'p>(_pool: AutoreleasePool<'p>, kind: Resize) -> Option<&'p AnyObject> {
    // SAFETY: a class method's answer to `respondsToSelector:`.
    let has =
        |sel: Sel| -> bool { unsafe { msg_send![class!(NSCursor), respondsToSelector: sel] } };
    // SAFETY: NSCursor's class methods with NSUInteger option arguments, asked only once NSCursor says it has them;
    // the cursors live in the caller's pool.
    unsafe {
        let cursor: *mut AnyObject = match kind {
            Resize::Column(d) if has(sel!(columnResizeCursorInDirections:)) => {
                msg_send![class!(NSCursor), columnResizeCursorInDirections: d]
            }
            Resize::Row(d) if has(sel!(rowResizeCursorInDirections:)) => {
                msg_send![class!(NSCursor), rowResizeCursorInDirections: d]
            }
            Resize::Frame(at, d) if has(sel!(frameResizeCursorFromPosition:inDirections:)) => {
                msg_send![class!(NSCursor), frameResizeCursorFromPosition: at, inDirections: d]
            }
            _ => return None,
        };
        cursor.as_ref()
    }
}

/// A macOS 15 resize cursor's arguments: `NSHorizontalDirections`, `NSVerticalDirections`, or
/// `NSCursorFrameResizePosition` with `NSCursorFrameResizeDirections` (each an NSUInteger bit set).
#[derive(Clone, Copy, Debug)]
pub(super) enum Resize {
    Column(usize),
    Row(usize),
    Frame(usize, usize),
}
