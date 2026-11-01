//! The share sheet on macOS (`share`): `NSSharingServicePicker` with the files' URLs, shown beside the anchor in the
//! view. AppKit runs the menu and the service the person picks; nothing here reads the files.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{NSSharingServicePicker, NSView};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSRectEdge, NSSize, NSURL};

use crate::share::Anchor;

thread_local! {
    /// The picker shown last (the main thread's), held until the next one shows: AppKit isn't asked to keep it alive
    /// while its menu and the service the person picked run.
    static SHOWN: RefCell<Option<Retained<NSSharingServicePicker>>> = const { RefCell::new(None) };
}

pub(crate) fn share_supported() -> bool {
    true
}

pub(crate) fn share_files(view: *mut c_void, anchor: Anchor, files: &[PathBuf]) -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }
    let urls: Vec<Retained<NSURL>> = files.iter().filter_map(NSURL::from_file_path).collect();
    if urls.is_empty() {
        return false;
    }
    // SAFETY: the caller hands a live NSView of the window on the main thread; retaining it keeps it alive here.
    let Some(view) = (unsafe { Retained::<NSView>::retain(view.cast()) }) else {
        return false;
    };
    let items = NSArray::from_retained_slice(&urls);
    // SAFETY: the items are file URLs, which the picker shares as files (NSURL is NSPasteboardWriting); an NSArray of
    // NSURL is an NSArray of objects.
    let picker = unsafe {
        NSSharingServicePicker::initWithItems(
            NSSharingServicePicker::alloc(),
            items.cast_unchecked::<AnyObject>(),
        )
    };
    let rect = NSRect::new(
        NSPoint::new(anchor.x, anchor.y),
        NSSize::new(anchor.width.max(1.0), anchor.height.max(1.0)),
    );
    SHOWN.set(Some(picker.clone()));
    picker.showRelativeToRect_ofView_preferredEdge(rect, &view, NSRectEdge::MaxY);
    true
}
