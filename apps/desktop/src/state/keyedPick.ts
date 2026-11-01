import { useCallback, useEffect, useRef, useState, type FocusEvent } from 'react';
import { registerBurst, SETTLE_MS } from './bursts';

/**
 * A radio group's arrow keys as one change: each press moves the checked choice at once (`shown`), and the last one is
 * picked `SETTLE_MS` after the run (or as focus leaves the group, it unmounts, an undo or the close comes), so a run of
 * presses is one undo step. A click picks at once.
 */
export function useKeyedPick<T>(value: T, onPick: (value: T) => void) {
  const [keyed, setKeyed] = useState<{ value: T } | null>(null);
  const pending = useRef<{ value: T } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const latest = useRef({ value, onPick });
  latest.current = { value, onPick };
  const settle = useCallback(() => {
    clearTimeout(timer.current);
    const run = pending.current;
    pending.current = null;
    setKeyed(null);
    if (run && run.value !== latest.current.value) latest.current.onPick(run.value);
    return run !== null;
  }, []);
  useEffect(() => registerBurst(() => settle()), [settle]);
  // Unmounted mid-run (the selection moved on): the run is kept.
  useEffect(() => () => void settle(), [settle]);
  const key = (next: T) => {
    pending.current = { value: next };
    setKeyed({ value: next });
    clearTimeout(timer.current);
    timer.current = setTimeout(settle, SETTLE_MS);
  };
  const pick = (next: T) => {
    clearTimeout(timer.current);
    pending.current = null;
    setKeyed(null);
    onPick(next);
  };
  /** Focus left the group: the run is picked now. */
  const onBlur = (e: FocusEvent<HTMLElement>) => {
    if (!e.currentTarget.contains(e.relatedTarget as Node | null)) settle();
  };
  return { shown: keyed ? keyed.value : value, key, pick, onBlur, settle };
}
