import { useLayoutEffect, useRef } from 'react';

/** How long after a button's meaning changes a press is taken as the tail of a double press on the old meaning. */
export const SETTLE_MS = 400;

/**
 * For a button that keeps its place (and the keyboard focus) as its meaning changes, Stop becoming Try again or Pause
 * all becoming Resume all: whether a press now is meant for what it says. Rust answers in milliseconds, so the second
 * click of a double click would otherwise undo the first (Try again restarting a copy Stop just deleted).
 */
export function usePressGuard(meaning: string): () => boolean {
  const said = useRef(meaning);
  const changedAt = useRef(-Infinity);
  useLayoutEffect(() => {
    if (said.current === meaning) return;
    said.current = meaning;
    changedAt.current = Date.now();
  }, [meaning]);
  return () => Date.now() - changedAt.current >= SETTLE_MS;
}
