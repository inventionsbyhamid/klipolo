import { useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { SETTLE_RESIZE_MS } from '../preview/useSettledSize';

/** The time the lanes render: the visible part plus one view's width each side. The scroll handler only sets state
 * when the view has moved half a view past that window, so scrolling re-renders a few times per screenful. */
export interface Viewport {
  fromUs: number;
  toUs: number;
  widthPx: number;
  scrollPx: number;
}

/** A window and the scale it was made for. */
interface ScaledViewport extends Viewport {
  pps: number;
}

/** How long a window is at `pps`: the view and one view each side, even where the timeline's start cuts it off. */
export function windowSpanUs(widthPx: number, pps: number, headerPx: number): number {
  return (3 * Math.max(1, widthPx - headerPx) * 1e6) / pps;
}

function windowAt(scrollPx: number, widthPx: number, pps: number, headerPx: number): ScaledViewport {
  const view = Math.max(1, widthPx - headerPx);
  const visFrom = (scrollPx / pps) * 1e6;
  const visTo = ((scrollPx + view) / pps) * 1e6;
  const viewUs = visTo - visFrom;
  return { fromUs: Math.max(0, visFrom - viewUs), toUs: visTo + viewUs, widthPx, scrollPx, pps };
}

/**
 * `nextScrollPx` is where a zoom that is about to render will scroll (its anchor), if one is: that render already
 * draws the new scale's window there, not the old window at the new scale (a ×2.7 wheel step would otherwise mount
 * 2.7× the blocks for one render). The layout effect then stores the same window.
 */
export function useViewport(
  ref: RefObject<HTMLElement | null>,
  pps: number,
  headerPx: number,
  nextScrollPx: number | null = null,
): Viewport {
  const [viewport, setViewport] = useState<ScaledViewport>({
    fromUs: 0,
    toUs: 0,
    widthPx: 0,
    scrollPx: 0,
    pps,
  });
  // The window on screen, for the observer: a width change holds it while it still covers the view. Declared before
  // the effect below, so it is current when that runs.
  const shown = useRef(viewport);
  useLayoutEffect(() => {
    shown.current = viewport;
  });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = (force: boolean) => {
      // Before the first layout (and in jsdom) a 1200 px view is assumed so something draws.
      const widthPx = el.clientWidth || 1200;
      const scrollPx = el.scrollLeft;
      const visFrom = (scrollPx / pps) * 1e6;
      const viewUs = (Math.max(1, widthPx - headerPx) / pps) * 1e6;
      const visTo = visFrom + viewUs;
      setViewport((current) => {
        // The window's start is clamped at 0, so the check is too (else every scroll near the start re-renders).
        const inside =
          current.fromUs <= Math.max(0, visFrom - viewUs / 2) && current.toUs >= visTo + viewUs / 2;
        if (!force && inside && current.widthPx === widthPx && current.pps === pps) return current;
        return windowAt(scrollPx, widthPx, pps, headerPx);
      });
    };
    update(true);
    const onScroll = () => update(false);
    let timer: ReturnType<typeof setTimeout> | undefined;
    // A height change (the timeline's bar, the window's height) keeps the window: nothing renders, and no state is
    // even set (a no-op set can still render once). A width change (a window drag) renders once, SETTLE_RESIZE_MS after
    // the last, while the lanes drawn still cover the view (they and the ruler stretch in CSS meanwhile); past them it
    // re-windows at once. A scroll (a shrink can clamp one) has its own event.
    const observer = new ResizeObserver(() => {
      clearTimeout(timer);
      const widthPx = el.clientWidth || 1200;
      const held = shown.current;
      if (widthPx === held.widthPx) return;
      const visTo = ((el.scrollLeft + Math.max(1, widthPx - headerPx)) / pps) * 1e6;
      if (visTo > held.toUs) update(false);
      else timer = setTimeout(() => update(false), SETTLE_RESIZE_MS);
    });
    observer.observe(el);
    el.addEventListener('scroll', onScroll, { passive: true });
    return () => {
      observer.disconnect();
      el.removeEventListener('scroll', onScroll);
      clearTimeout(timer);
    };
  }, [ref, pps, headerPx]);
  if (viewport.pps !== pps && viewport.widthPx > 0)
    return windowAt(nextScrollPx ?? viewport.scrollPx, viewport.widthPx, pps, headerPx);
  return viewport;
}
