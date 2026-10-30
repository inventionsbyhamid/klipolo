//! Which frame to present next: frames are shown at their due time, never just because they arrived.
//! Pure, so it is tested without a display. `src/lib/framePick.ts` is the socket path's twin.

/// The host clock in ns (`klipolo_platform::clock`): on macOS the clock `MTLDrawable.presentedTime` and
/// `presentDrawable:atTime:` use (CLOCK_UPTIME_RAW).
pub use klipolo_platform::clock::host_ns;

use crate::frames::trailer::now_ms;

/// A frame waiting to be shown: the order it was queued in and when it is due (host ns; 0: at once).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waiting {
    pub order: u64,
    pub due_ns: u64,
}

/// The index of the newest waiting frame due by the refresh at `target_ns`, within half a refresh so a frame lands on
/// the refresh nearest its due time. Stills (due 0) are always due. `None`: keep what is on screen.
pub fn pick(waiting: &[Waiting], target_ns: u64, refresh_ns: u64) -> Option<usize> {
    let by = target_ns.saturating_add(refresh_ns / 2);
    waiting
        .iter()
        .enumerate()
        .filter(|(_, w)| w.due_ns <= by)
        .max_by_key(|(_, w)| w.order)
        .map(|(i, _)| i)
}

/// What the native presenter commits next, and when it should reach the screen:
/// - the newest frame due already (stills, or frames that came late): at once, and the older ones are skipped;
/// - otherwise the oldest frame due within `horizon_ns`: scheduled for half a refresh before its due time, so it lands
///   on the refresh nearest that time (`presentDrawable:atTime:` shows it on the first refresh after).
///
/// Returns its index and the host time to present at (`None`: at once).
pub fn next(
    waiting: &[Waiting],
    now_ns: u64,
    refresh_ns: u64,
    horizon_ns: u64,
) -> Option<(usize, Option<u64>)> {
    if let Some(i) = pick(waiting, now_ns, refresh_ns) {
        return Some((i, None));
    }
    let by = now_ns.saturating_add(horizon_ns);
    waiting
        .iter()
        .enumerate()
        .filter(|(_, w)| w.due_ns <= by)
        .min_by_key(|(_, w)| w.order)
        .map(|(i, w)| (i, Some(w.due_ns.saturating_sub(refresh_ns / 2))))
}

/// The refresh a scheduled playback frame should reach the glass on (host ns), keeping the cadence steady: a frame
/// due a whole number of refreshes after the previous one goes exactly that many refreshes later, as long as that stays
/// within ¾ of a refresh of its due time (else it goes at its due time). Without this, frames due near the middle
/// between two refreshes flip between them and 30 fps on 60 Hz judders 1:3 instead of 2:2. `prev`: the previous
/// frame's refresh and due time in this run of playback (`None`: the first frame).
pub fn cadence(due_ns: u64, prev: Option<(u64, u64)>, refresh_ns: u64) -> u64 {
    let r = refresh_ns as f64;
    match prev {
        Some((prev_refresh, prev_due)) if due_ns > prev_due => {
            let steps = ((due_ns - prev_due) as f64 / r).round().max(1.0);
            let steady = (prev_refresh as f64 + steps * r) as u64;
            if steady.abs_diff(due_ns) as f64 <= 0.75 * r {
                steady
            } else {
                due_ns
            }
        }
        _ => due_ns,
    }
}

/// A frame's due time (`FrameMeta::due_ms`, epoch ms; 0 = at once) on the host clock (0 = at once).
pub fn due_host_ns(due_ms: f64) -> u64 {
    if due_ms <= 0.0 {
        return 0;
    }
    let ahead_ns = ((due_ms - now_ms()) * 1e6).round() as i64;
    host_ns().saturating_add_signed(ahead_ns).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFRESH: u64 = 16_666_667;
    const MS: u64 = 1_000_000;

    fn w(order: u64, due_ms: u64) -> Waiting {
        Waiting {
            order,
            due_ns: due_ms * MS,
        }
    }

    #[test]
    fn frames_are_scheduled_in_order_for_their_refresh() {
        // 30 fps frames due every 33.3 ms on a 60 Hz display, committed up to 25 ms ahead.
        let waiting = [w(1, 1000), w(2, 1033), w(3, 1067)];
        let horizon = REFRESH * 3 / 2;
        assert_eq!(
            next(&waiting, 960 * MS, REFRESH, horizon),
            None,
            "nothing due soon: wait"
        );
        assert_eq!(
            next(&waiting, 980 * MS, REFRESH, horizon),
            Some((0, Some(1000 * MS - REFRESH / 2))),
            "the oldest, scheduled for the refresh nearest its due time"
        );
        assert_eq!(
            next(&waiting, 1040 * MS, REFRESH, horizon),
            Some((1, None)),
            "came late: shown at once, and the frame due earlier is skipped"
        );
    }

    #[test]
    fn thirty_fps_keeps_a_two_two_cadence_on_sixty_hz_despite_jitter() {
        // Due times wobbling ±2 ms (the sound clock and the render thread's timing): scheduled by due time alone, a
        // frame near the middle between refreshes flips between them; the cadence lock keeps every step at exactly
        // two refreshes.
        let start = 1_000 * MS;
        let mut prev = None;
        let mut refreshes = Vec::new();
        for k in 0..60u64 {
            let wobble = [0i64, 2, -2, 1, -1][k as usize % 5] * MS as i64;
            let due = (start + k * 2 * REFRESH).saturating_add_signed(wobble);
            let refresh = cadence(due, prev, REFRESH);
            assert!(refresh.abs_diff(due) <= REFRESH * 3 / 4);
            refreshes.push(refresh);
            prev = Some((refresh, due));
        }
        assert!(refreshes.windows(2).all(|w| w[1] - w[0] == 2 * REFRESH));
    }

    #[test]
    fn a_still_shows_at_once_and_wins_over_older_frames() {
        let waiting = [w(1, 5000), w(2, 0)];
        assert_eq!(next(&waiting, 1, REFRESH, REFRESH), Some((1, None)));
        assert_eq!(next(&[], 1, REFRESH, REFRESH), None);
    }
}
