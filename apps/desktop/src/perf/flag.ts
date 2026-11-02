/**
 * Whether the page's perf readout is on: the app sets `window.__KLIPOLO_PAGE_PERF__` before the page loads when it is
 * launched with `KLIPOLO_PAGE_PERF=1` (`commands/page_perf.rs`). Off in every other launch.
 */
export function pagePerfOn(): boolean {
  return typeof window !== 'undefined' && (window as { __KLIPOLO_PAGE_PERF__?: unknown }).__KLIPOLO_PAGE_PERF__ === true;
}
