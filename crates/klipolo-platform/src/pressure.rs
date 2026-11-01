//! System memory pressure: the OS backend records the level, and the decoder pool and the compositor read it each
//! frame:
//! - warning: the decoder budget halves, nothing is warmed ahead, and images and LUTs the frame doesn't draw go;
//! - critical: also only the current frame's decoders stay open.
//!
//! Text textures are per frame already and the glyph atlas is trimmed after each text. Dropping references to
//! IOSurfaces and textures doesn't touch their pages, so shrinking is safe under pressure (libdispatch's advice:
//! shrink, don't traverse old caches).
//!
//! The open project's undo history also shrinks to 50 steps at a warning, as soon as the level changes (`on_change`).
//!
//! macOS listens with a dispatch source. Windows (later: `CreateMemoryResourceNotification`, the DXGI budget) and Linux
//! (later: PSI) stay Normal for now.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, PoisonError};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pressure {
    Normal,
    Warning,
    Critical,
}

static LEVEL: AtomicU8 = AtomicU8::new(0);

type Listener = Box<dyn Fn(Pressure) + Send + Sync>;
static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());

/// The latest level the system reported (Normal until it says otherwise).
pub fn level() -> Pressure {
    match LEVEL.load(Ordering::Relaxed) {
        0 => Pressure::Normal,
        1 => Pressure::Warning,
        _ => Pressure::Critical,
    }
}

/// Calls `listener` with each new level from now on (not the current one), on the backend's thread (macOS: a utility
/// dispatch queue). Keep it short; it may take a lock. Nothing polls: this is how a change reaches state that isn't
/// read on every frame.
pub fn on_change(listener: impl Fn(Pressure) + Send + Sync + 'static) {
    listeners().push(Box::new(listener));
}

fn listeners() -> std::sync::MutexGuard<'static, Vec<Listener>> {
    LISTENERS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The backend's report: 0 normal, 1 warning, 2 critical. A changed level goes to every `on_change` listener.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn set_level(level: u8) {
    if LEVEL.swap(level, Ordering::Relaxed) == level {
        return;
    }
    let now = self::level();
    for listener in listeners().iter() {
        listener(now);
    }
}

/// Starts listening (once per process; later calls do nothing).
pub fn watch() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(crate::imp::watch_pressure);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn listeners_hear_each_change_once() {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let sink = heard.clone();
        on_change(move |level| sink.lock().unwrap().push(level));
        set_level(1);
        set_level(1);
        set_level(0);
        assert_eq!(
            *heard.lock().unwrap(),
            [Pressure::Warning, Pressure::Normal]
        );
        assert_eq!(level(), Pressure::Normal);
    }
}
