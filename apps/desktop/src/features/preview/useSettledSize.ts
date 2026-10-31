import { useLayoutEffect, useState, type RefObject } from 'react';
import type { Size } from '../../lib/preview';
import { useStableCallback } from '../../lib/useStableCallback';
import { useLayout } from '../../state/layout';

/** How long a size must hold still before it counts as settled (a window resize; a drag settles when it ends). */
export const SETTLE_RESIZE_MS = 120;

/**
 * The element's size, settled: the first measurement at once (and again on the first frame, at once), then each change
 * once it stops: 120 ms after the last one, or as soon as a separator or timeline drag ends (`useLayout.resizing`),
 * never during one. `onSettling` hears each change of a run with the size now (`true`), then the run's end with the
 * settled size (`false`): the stage hides its handles meanwhile, with no render.
 */
export function useSettledSize(
  ref: RefObject<HTMLElement | null>,
  onSettling?: (settling: boolean, width: number, height: number) => void,
): Size {
  const [size, setSize] = useState<Size>({ width: 0, height: 0 });
  const settling = useStableCallback((on: boolean, width: number, height: number) =>
    onSettling?.(on, width, height),
  );
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    let last: Size = { width: el.clientWidth, height: el.clientHeight };
    let busy = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const take = () => {
      const now = { width: el.clientWidth, height: el.clientHeight };
      last = now;
      return now;
    };
    const settle = () => {
      clearTimeout(timer);
      if (!busy) return;
      busy = false;
      const now = take();
      settling(false, now.width, now.height);
      setSize((s) => (s.width === now.width && s.height === now.height ? s : now));
    };
    setSize(last);
    // A panel that folds in this same commit (a saved-folded chat, a small window's squeeze) folds after this
    // measure, in a parent's layout effect: look again on the first frame, before the observer's first delivery, and
    // take that size at once.
    const frame = requestAnimationFrame(() => {
      if (busy || (el.clientWidth === last.width && el.clientHeight === last.height)) return;
      setSize(take());
    });
    // Per frame of a resize: two reads and no allocation.
    const observer = new ResizeObserver(() => {
      const width = el.clientWidth;
      const height = el.clientHeight;
      if (!busy && width === last.width && height === last.height) return;
      busy = true;
      settling(true, width, height);
      clearTimeout(timer);
      if (!useLayout.getState().resizing) timer = setTimeout(settle, SETTLE_RESIZE_MS);
    });
    observer.observe(el);
    // A drag that starts while a window resize settles holds it too: its end settles both.
    const off = useLayout.subscribe((s, was) => {
      if (s.resizing && !was.resizing) clearTimeout(timer);
      else if (was.resizing && !s.resizing) settle();
    });
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      off();
      clearTimeout(timer);
    };
  }, [ref, settling]);
  return size;
}
