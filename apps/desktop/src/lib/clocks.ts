/**
 * The engine stamps frames with wall-clock time (ms since the Unix epoch, Rust's `SystemTime`); the page's animation
 * frames and the playback clock use `performance.now()`. Convert between them through `Date.now()`, never through
 * `performance.timeOrigin + performance.now()`: WebKit's performance clock stops while the Mac sleeps, so after a
 * sleep that sum lags the wall clock by however long the Mac slept.
 */

/** Wall-clock time (epoch ms) at performance time `perfMs`. */
export function epochAt(perfMs: number): number {
  return Date.now() + (perfMs - performance.now());
}

/** Performance time at wall-clock time `epochMs`. */
export function perfAt(epochMs: number): number {
  return performance.now() + (epochMs - Date.now());
}
