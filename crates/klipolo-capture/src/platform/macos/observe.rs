//! Notification observers for the shell (`power`, `focus`), removed when dropped.
//! - `Observers`: blocks on an `NSNotificationCenter` (NSWorkspace's, the process's), run on the main queue.
//! - `Distributed`: the distributed center's notices through Core Foundation, delivered at once. AppKit holds distributed
//!   notices back while an app isn't active, and the worker, an accessory app, almost never is; Core Foundation's
//!   `DeliverImmediately` is the only way to hear them as they happen.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::mpsc::SyncSender;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_core_foundation::{
    CFDictionary, CFNotificationCenter, CFNotificationName, CFNotificationSuspensionBehavior,
    CFRetained, CFString,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSOperationQueue,
};

use crate::shell::ShellInput;

type Token = Retained<ProtocolObject<dyn NSObjectProtocol>>;

/// Block observers, each on its center.
#[derive(Default)]
pub(super) struct Observers(Vec<(Retained<NSNotificationCenter>, Token)>);

impl Observers {
    /// Calls `on` on the main queue for each `name` posted to `center`.
    pub(super) fn add(
        &mut self,
        center: &Retained<NSNotificationCenter>,
        name: &NSNotificationName,
        on: impl Fn(&NSNotification) + Send + Sync + 'static,
    ) {
        let block = RcBlock::new(move |note: NonNull<NSNotification>| {
            // SAFETY: the center passes a notification that lives for the call.
            on(unsafe { note.as_ref() });
        });
        // SAFETY: `on` is `Send + Sync`, as a block the center may keep and call requires; no object filter.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(name),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &block,
            )
        };
        self.0.push((center.clone(), token));
    }
}

impl Drop for Observers {
    fn drop(&mut self) {
        for (center, token) in self.0.drain(..) {
            // SAFETY: a token this center gave; removed once.
            unsafe { center.removeObserver(token.as_ref()) };
        }
    }
}

/// One distributed notice's input for the session, and where it goes.
struct Notice {
    input: SyncSender<ShellInput>,
    send: ShellInput,
}

/// Distributed notices (by name) that each send their input, at once, from the main thread's run loop.
pub(super) struct Distributed {
    center: Option<CFRetained<CFNotificationCenter>>,
    #[allow(
        clippy::vec_box,
        reason = "each notice's address is its observer's, so it must never move"
    )]
    notices: Vec<Box<Notice>>,
}

impl Distributed {
    pub(super) fn new(input: &SyncSender<ShellInput>, notices: &[(&str, ShellInput)]) -> Self {
        let center = CFNotificationCenter::distributed_center();
        let mut boxed = Vec::with_capacity(notices.len());
        for (name, send) in notices {
            let Some(center) = &center else { break };
            let notice = Box::new(Notice {
                input: input.clone(),
                send: send.clone(),
            });
            let observer = (&*notice as *const Notice).cast::<c_void>();
            // SAFETY: `observer` points at the boxed notice, which lives until `drop` removes this observer; the
            // callback reads it only as a `Notice`.
            unsafe {
                center.add_observer(
                    observer,
                    Some(posted),
                    Some(&CFString::from_str(name)),
                    std::ptr::null(),
                    CFNotificationSuspensionBehavior::DeliverImmediately,
                );
            }
            boxed.push(notice);
        }
        Self {
            center,
            notices: boxed,
        }
    }
}

impl Drop for Distributed {
    fn drop(&mut self) {
        let Some(center) = &self.center else { return };
        for notice in &self.notices {
            // SAFETY: the observer pointer `new` registered; after this the callback can't see the notice.
            unsafe { center.remove_every_observer((&**notice as *const Notice).cast()) };
        }
    }
}

/// A distributed notice was posted: its observer is a `Notice` (`Distributed::new`).
unsafe extern "C-unwind" fn posted(
    _center: *mut CFNotificationCenter,
    observer: *mut c_void,
    _name: *const CFNotificationName,
    _object: *const c_void,
    _info: *const CFDictionary,
) {
    // SAFETY: `Distributed::new` registered a boxed `Notice` as the observer and removes it before freeing it.
    let Some(notice) = (unsafe { observer.cast::<Notice>().as_ref() }) else {
        return;
    };
    let _ = notice.input.send(notice.send.clone());
}
