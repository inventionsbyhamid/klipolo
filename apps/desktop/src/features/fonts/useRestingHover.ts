import { useCallback, useEffect, useLayoutEffect, useRef } from 'react';

/**
 * How long the pointer rests on a row before its font is shown on the text: a sweep down the
 * menu dry-runs only the row it stops on, never one engine build per row passed.
 */
export const HOVER_SETTLE_MS = 100;

/**
 * The font menu's hover preview: `rest(n)` shows row `n` once the pointer has stayed on it `HOVER_SETTLE_MS`, through
 * the latest `preview` (the listing may have moved on meanwhile), and a newer `rest` or `stop` cancels it. The menu
 * stops it when it closes, picks, or its query changes, and the arrow keys preview at once instead.
 */
export function useRestingHover(preview: (n: number) => void) {
  const resting = useRef<ReturnType<typeof setTimeout> | null>(null);
  const latest = useRef(preview);
  useLayoutEffect(() => {
    latest.current = preview;
  });
  const stop = useCallback(() => {
    if (resting.current !== null) clearTimeout(resting.current);
    resting.current = null;
  }, []);
  useEffect(() => stop, [stop]);
  const rest = useCallback(
    (n: number) => {
      stop();
      resting.current = setTimeout(() => {
        resting.current = null;
        latest.current(n);
      }, HOVER_SETTLE_MS);
    },
    [stop],
  );
  return { rest, stop };
}
