//! Klipolo's own windows on screen during a take: the camera bubble and the HUD, which a display's
//! filter leaves out of the picture, set on the main thread as they show, move and hide, and read on the screen
//! stream's queue (`screen::feed`) to tell a frame that changed only them from a real one (`common::our_damage`). Their
//! frames are AppKit's global points (bottom-left origin), as `NSEvent.mouseLocation` is. A copy: nothing allocates.

use std::sync::{Mutex, PoisonError};

use objc2_foundation::NSRect;

use crate::platform::common::sck::CgRect;

/// One of Klipolo's windows over the recorded screen.
#[derive(Clone, Copy, Debug)]
pub(super) enum Own {
    Bubble = 0,
    Hud = 1,
}

static SHOWN: Mutex<[Option<CgRect>; 2]> = Mutex::new([None; 2]);

/// `which` is on screen at `frame`, or hidden (None).
pub(super) fn set(which: Own, frame: Option<NSRect>) {
    let rect = frame.map(|f| CgRect {
        x: f.origin.x,
        y: f.origin.y,
        w: f.size.width,
        h: f.size.height,
    });
    SHOWN.lock().unwrap_or_else(PoisonError::into_inner)[which as usize] = rect;
}

/// The windows on screen now.
pub(super) fn shown() -> [Option<CgRect>; 2] {
    *SHOWN.lock().unwrap_or_else(PoisonError::into_inner)
}
