//! The displays, listed without capture: Core Graphics' active displays, each with its current mode's pixels (what a
//! recording of it holds) and refresh rate, the main display first (preflight's defaults are the first's). Their names
//! come from `NSScreen`, which AppKit answers on the main thread only: read there directly, else asked of the main queue
//! and waited for briefly, so a main thread that isn't running its loop costs the names, never a hang.

use std::collections::BTreeMap;
use std::sync::mpsc::sync_channel;
use std::time::Duration;

use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayCopyDisplayMode, CGDisplayMode, CGGetActiveDisplayList,
    CGMainDisplayID,
};
use objc2_foundation::{NSNumber, ns_string};

use crate::backend::DisplayInfo;

/// More displays than any Mac drives.
const MOST_DISPLAYS: usize = 32;
/// How long the names may take from the main thread.
const NAMES_WAIT: Duration = Duration::from_millis(250);
/// A mode that doesn't say its rate (some built-in panels answer 0).
const DEFAULT_HZ: u32 = 60;

pub(super) fn list() -> Vec<DisplayInfo> {
    let names = screen_names();
    active().into_iter().map(|id| display(id, &names)).collect()
}

/// The active displays' ids, the main display first.
fn active() -> Vec<CGDirectDisplayID> {
    let mut ids = [0 as CGDirectDisplayID; MOST_DISPLAYS];
    let mut count = 0_u32;
    // SAFETY: room for `MOST_DISPLAYS` ids, and the count's out-pointer; the call writes at most that many.
    let error =
        unsafe { CGGetActiveDisplayList(MOST_DISPLAYS as u32, ids.as_mut_ptr(), &mut count) };
    if error.0 != 0 {
        tracing::warn!(
            error = error.0,
            "displays: the active list couldn't be read"
        );
        return Vec::new();
    }
    let mut ids = ids[..(count as usize).min(MOST_DISPLAYS)].to_vec();
    let main = CGMainDisplayID();
    if let Some(at) = ids.iter().position(|&id| id == main) {
        ids[..=at].rotate_right(1);
    }
    ids
}

fn display(id: CGDirectDisplayID, names: &BTreeMap<u32, String>) -> DisplayInfo {
    let mode = CGDisplayCopyDisplayMode(id);
    let mode = mode.as_deref();
    let hz = CGDisplayMode::refresh_rate(mode).round();
    DisplayInfo {
        id: id.to_string(),
        name: names.get(&id).cloned().unwrap_or_default(),
        width: u32::try_from(CGDisplayMode::pixel_width(mode)).unwrap_or(0),
        height: u32::try_from(CGDisplayMode::pixel_height(mode)).unwrap_or(0),
        refresh_hz: if hz >= 1.0 { hz as u32 } else { DEFAULT_HZ },
    }
}

/// Each screen's name by its display id (empty when the main thread didn't answer in time).
fn screen_names() -> BTreeMap<u32, String> {
    if let Some(mtm) = MainThreadMarker::new() {
        return names_on(mtm);
    }
    let (tx, rx) = sync_channel(1);
    let mut tx = Some(tx);
    cidre::dispatch::Queue::main().async_mut(move || {
        if let (Some(tx), Some(mtm)) = (tx.take(), MainThreadMarker::new()) {
            let _ = tx.try_send(names_on(mtm));
        }
    });
    rx.recv_timeout(NAMES_WAIT).unwrap_or_else(|_| {
        tracing::warn!("displays: the main thread didn't name the screens in time");
        BTreeMap::new()
    })
}

fn names_on(mtm: MainThreadMarker) -> BTreeMap<u32, String> {
    objc2::rc::autoreleasepool(|_| {
        (NSScreen::screens(mtm).iter())
            .filter_map(|screen| {
                let number = screen
                    .deviceDescription()
                    .objectForKey(ns_string!("NSScreenNumber"))?;
                let id = number.downcast_ref::<NSNumber>()?.unsignedIntValue();
                Some((id, screen.localizedName().to_string()))
            })
            .collect()
    })
}
