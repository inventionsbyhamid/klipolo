import { useEffect, type RefObject } from 'react';
import { clockAnchor, clockUs, onClock } from '../../state/playClock';
import { usePlayback } from '../../state/playback';
import { useUi } from '../../state/ui';
import { usToPx } from './scale';

/** How close to the right edge (the left one, going backward) the playhead gets before the view pages (px). */
const MARGIN = 16;

/**
 * Keeps the playhead on screen in the scrolling timeline: it is in view when the timeline opens, and while playing the
 * view pages forward when the playhead reaches the right edge (backward, back when it reaches the left edge, leaving it
 * near the right one). Zooms keep their own anchor (`zoomTimeline`: the pointer, else the playhead on screen); while
 * playing, the follow re-checks against the zoom's new scroll, so it pages at the new edge and only moves the view if
 * the zoom put the playhead off screen. `headerPx` is the sticky header column. Nothing runs per frame: one timer fires
 * when the playhead will reach the edge, rescheduled when the clock is re-anchored or the zoom changes. Each check
 * reads the view's scroll and width then (a few times per screenful), so it never works from a position a zoom or the
 * user has since changed.
 */
export function useFollowPlayhead(ref: RefObject<HTMLElement | null>, pps: number, headerPx: number) {
  useEffect(() => {
    const el = ref.current;
    const view = el ? el.clientWidth - headerPx : 0;
    if (!el || view <= 0) return;
    const x = usToPx(usePlayback.getState().playheadUs, useUi.getState().pxPerSecond);
    if (x < el.scrollLeft || x > el.scrollLeft + view) el.scrollLeft = Math.max(0, x - view / 2);
  }, [ref, headerPx]);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const follow = () => {
      clearTimeout(timer);
      const el = ref.current;
      if (!el || !usePlayback.getState().playing || !clockAnchor().running) return;
      const view = el.clientWidth - headerPx;
      if (view <= 0) return;
      const { rate } = clockAnchor();
      const x = usToPx(clockUs(), pps);
      let scroll = el.scrollLeft;
      let toEdgePx: number;
      if (rate < 0) {
        if (x > scroll + view || (scroll > 0 && x < scroll + MARGIN)) {
          scroll = Math.max(0, x + MARGIN - view);
          el.scrollLeft = scroll;
        }
        // At the start there is nothing further back to show.
        if (scroll === 0) return;
        toEdgePx = x - scroll - MARGIN;
      } else {
        if (x < scroll || x > scroll + view - MARGIN) {
          scroll = Math.max(0, x - MARGIN);
          el.scrollLeft = scroll;
        }
        toEdgePx = scroll + view - MARGIN - x;
      }
      // The playhead covers the distance |rate| times as fast.
      timer = setTimeout(follow, Math.max(50, ((toEdgePx / pps) * 1000) / Math.abs(rate)));
    };
    follow();
    const offClock = onClock(follow);
    const offStore = usePlayback.subscribe((s, prev) => {
      if (s.playing !== prev.playing) follow();
    });
    return () => {
      clearTimeout(timer);
      offClock();
      offStore();
    };
  }, [ref, pps, headerPx]);
}
