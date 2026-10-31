//! Named failpoints, for the kill sweep, the power-loss replay and the writer's own tests. The
//! pattern of Cap's `fail` crate (`crates/fail/src/lib.rs` at d327051: named points compiled only into test builds;
//! pattern only, no code), without `inventory`.
//!
//! - **Debug and `synthetic` builds only.** In a release build `fail_point!` is `Ok(())`, `fail_armed!` is `false`,
//!   `set` does nothing and `KLIPOLO_FAIL` isn't read, so no point's name reaches the binary (a release
//!   looks for `writer.midAppend`).
//! - **Arming.** `set(name, Some(action))` arms a point for the whole process (tests: a test that arms one runs in its
//!   own test binary, or uses a name no other test reaches). `KLIPOLO_FAIL` arms them when the first point is reached:
//!   `KLIPOLO_FAIL="writer.beforeBarrier=abort,writer.midAppend=error:full,remux.midFlips=sleep:500"`, for a child
//!   process such as the kill sweep's `synthetic-take`. Errors are `full`, `io`, `readOnly`, `denied` and
//!   `interrupted`.
//! - **The points:** `writer.beforeAppend`; `writer.midAppend` (writes the first half of the bytes, then acts);
//!   `writer.beforePush`, `writer.beforeDurable`, `writer.beforeBarrier` and `writer.afterBarrier` (the tick's steps);
//!   `remux.afterTail` (the tail is written and synced) and `remux.midFlips` (half the flips are written);
//!   `recover.levels` (a crashed sound run's levels read).
//!
//! Cost when nothing is armed: one relaxed atomic load per point.

use std::io;
use std::time::Duration;

/// What an armed point does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailAction {
    /// Ends the process at once, as a `kill -9` would (`std::process::abort`).
    Abort,
    /// The step fails with this kind of error.
    Error(io::ErrorKind),
    /// The step waits this long first (a slow drive).
    Sleep(Duration),
}

/// Arms `name` with `action`, or disarms it with None. Does nothing in a release build.
pub fn set(name: &'static str, action: Option<FailAction>) {
    #[cfg(any(debug_assertions, feature = "synthetic"))]
    imp::set(name, action);
    #[cfg(not(any(debug_assertions, feature = "synthetic")))]
    let _ = (name, action);
}

/// `Ok(())`, unless the point is armed: then it aborts, sleeps, or gives its error. A release build compiles it out.
macro_rules! fail_point {
    ($name:literal) => {{
        #[cfg(any(debug_assertions, feature = "synthetic"))]
        let hit = $crate::fail::imp::eval($name);
        #[cfg(not(any(debug_assertions, feature = "synthetic")))]
        let hit: ::std::io::Result<()> = ::std::result::Result::Ok(());
        hit
    }};
}

/// Whether the point is armed (`writer.midAppend` writes half its bytes first); `false` in a release build.
macro_rules! fail_armed {
    ($name:literal) => {{
        #[cfg(any(debug_assertions, feature = "synthetic"))]
        let armed = $crate::fail::imp::armed($name);
        #[cfg(not(any(debug_assertions, feature = "synthetic")))]
        let armed = false;
        armed
    }};
}

pub(crate) use {fail_armed, fail_point};

#[cfg(any(debug_assertions, feature = "synthetic"))]
pub(crate) mod imp {
    use std::collections::BTreeMap;
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, MutexGuard, Once, PoisonError};
    use std::time::Duration;

    use super::FailAction;

    static POINTS: Mutex<BTreeMap<String, FailAction>> = Mutex::new(BTreeMap::new());
    /// Any point is armed: the fast path when none is.
    static ARMED: AtomicBool = AtomicBool::new(false);
    static FROM_ENV: Once = Once::new();

    fn points() -> MutexGuard<'static, BTreeMap<String, FailAction>> {
        FROM_ENV.call_once(|| {
            let spec = std::env::var("KLIPOLO_FAIL").unwrap_or_default();
            let mut points = POINTS.lock().unwrap_or_else(PoisonError::into_inner);
            points.extend(parse(&spec));
            ARMED.store(!points.is_empty(), Ordering::Release);
        });
        POINTS.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set(name: &str, action: Option<FailAction>) {
        let mut points = points();
        match action {
            Some(action) => points.insert(name.to_owned(), action),
            None => points.remove(name),
        };
        ARMED.store(!points.is_empty(), Ordering::Release);
    }

    fn action(name: &str) -> Option<FailAction> {
        if FROM_ENV.is_completed() && !ARMED.load(Ordering::Acquire) {
            return None;
        }
        points().get(name).copied()
    }

    pub(crate) fn armed(name: &str) -> bool {
        action(name).is_some()
    }

    pub(crate) fn eval(name: &str) -> io::Result<()> {
        match action(name) {
            None => Ok(()),
            Some(FailAction::Abort) => {
                eprintln!("failpoint {name}: abort");
                std::process::abort()
            }
            Some(FailAction::Error(kind)) => Err(io::Error::new(kind, format!("failpoint {name}"))),
            Some(FailAction::Sleep(d)) => {
                std::thread::sleep(d);
                Ok(())
            }
        }
    }

    /// `name=action` pairs between commas; anything it can't read is logged and skipped.
    pub(crate) fn parse(spec: &str) -> Vec<(String, FailAction)> {
        let read = |action: &str| -> Option<FailAction> {
            let (verb, arg) = action.split_once(':').unwrap_or((action, ""));
            Some(match (verb, arg) {
                ("abort", "") => FailAction::Abort,
                ("sleep", ms) => FailAction::Sleep(Duration::from_millis(ms.parse().ok()?)),
                ("error", kind) => FailAction::Error(match kind {
                    "full" => io::ErrorKind::StorageFull,
                    "io" => io::ErrorKind::Other,
                    "readOnly" => io::ErrorKind::ReadOnlyFilesystem,
                    "denied" => io::ErrorKind::PermissionDenied,
                    "interrupted" => io::ErrorKind::Interrupted,
                    _ => return None,
                }),
                _ => return None,
            })
        };
        spec.split(',')
            .map(str::trim)
            .filter(|pair| !pair.is_empty())
            .filter_map(|pair| {
                let parsed = pair
                    .split_once('=')
                    .filter(|(name, _)| !name.trim().is_empty())
                    .and_then(|(name, action)| {
                        Some((name.trim().to_owned(), read(action.trim())?))
                    });
                if parsed.is_none() {
                    tracing::warn!(pair, "KLIPOLO_FAIL: skipped a failpoint it can't read");
                }
                parsed
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_are_read_from_the_variable_and_act_when_armed() {
        assert_eq!(
            imp::parse(
                "writer.beforeBarrier=abort, remux.midFlips=sleep:500,writer.midAppend=error:full,x=y,=abort"
            ),
            [
                ("writer.beforeBarrier".to_owned(), FailAction::Abort),
                (
                    "remux.midFlips".to_owned(),
                    FailAction::Sleep(Duration::from_millis(500))
                ),
                (
                    "writer.midAppend".to_owned(),
                    FailAction::Error(io::ErrorKind::StorageFull)
                ),
            ]
        );
        // A name no other test reaches, so tests running beside this one are untouched.
        assert!(fail_point!("test.only").is_ok() && !fail_armed!("test.only"));
        set(
            "test.only",
            Some(FailAction::Error(io::ErrorKind::StorageFull)),
        );
        assert!(fail_armed!("test.only"));
        assert_eq!(
            fail_point!("test.only").unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        set("test.only", None);
        assert!(fail_point!("test.only").is_ok());
    }
}
