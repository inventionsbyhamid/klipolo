import { useCallback, useEffect, useRef } from 'react';
import { useStableCallback } from '../../lib/useStableCallback';
import { registerBurst, SETTLE_MS } from '../../state/bursts';

/**
 * A run of colour keys (arrows on the colour area, hue or opacity, or a VoiceOver or Voice Control step) settling into
 * one pick: `keyed` holds the newest colour and commits it `SETTLE_MS` after the last key; `flush`
 * commits it at once (the popover closing), and so do an undo, redo, export or close (`flushBursts`) and unmounting.
 * Each of these calls the newest `commit` (kept in a ref, assigned every render), so a settle flushed after a re-render
 * never commits through a stale closure (an older style). With nothing pending they commit nothing. `drop` forgets the
 * pending colour (another pick replaces it: one undo step, the pick) and says whether there was one.
 */
export function useKeySettle(commit: (color: string) => void): {
  keyed(color: string): void;
  flush(): void;
  drop(): boolean;
} {
  const commitRef = useStableCallback(commit);
  const pending = useRef<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);

  /** Commits the pending colour; true when there was one. */
  const send = useCallback((): boolean => {
    clearTimeout(timer.current);
    const color = pending.current;
    pending.current = null;
    if (color === null) return false;
    commitRef(color);
    return true;
  }, [commitRef]);

  useEffect(() => {
    const stop = registerBurst(() => send());
    return () => {
      stop();
      // Unmounted mid-settle: keep what was done.
      send();
    };
  }, [send]);

  const keyed = useCallback(
    (color: string) => {
      pending.current = color;
      clearTimeout(timer.current);
      timer.current = setTimeout(send, SETTLE_MS);
    },
    [send],
  );
  const flush = useCallback(() => void send(), [send]);
  const drop = useCallback(() => {
    clearTimeout(timer.current);
    const had = pending.current !== null;
    pending.current = null;
    return had;
  }, []);
  return { keyed, flush, drop };
}
