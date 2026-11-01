import { type RefObject, useLayoutEffect } from 'react';
import { boxPixels } from '../../lib/recorder/boxPixels';
import { setLivePixels } from '../../state/recorderPreviews';

/** A window being resized sends many sizes: the live picture follows once the box has held still this long. */
export const SETTLE_MS = 200;

/**
 * The live picture is made for the preview box `ref` shows it in, in device pixels (`setLivePixels`), so it is as
 * sharp as the box: measured as the box shows (before the previews are asked for), again once a resize settles, and
 * at once when the window moves to a display of another scale (its CSS size is the same, its pixels aren't).
 */
export function useLivePixels(ref: RefObject<HTMLElement | null>): void {
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setLivePixels(boxPixels(el.clientWidth, el.clientHeight, window.devicePixelRatio || 1));
    let timer: ReturnType<typeof setTimeout> | undefined;
    let density: MediaQueryList | null = null;
    // The display's scale now: a change means another display, whose own scale is watched from then on.
    const watch = () => {
      density?.removeEventListener('change', moved);
      density = window.matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
      density.addEventListener('change', moved);
    };
    function moved() {
      watch();
      measure();
    }
    measure();
    watch();
    const observer = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(measure, SETTLE_MS);
    });
    observer.observe(el);
    return () => {
      clearTimeout(timer);
      observer.disconnect();
      density?.removeEventListener('change', moved);
      setLivePixels(null);
    };
  }, [ref]);
}
