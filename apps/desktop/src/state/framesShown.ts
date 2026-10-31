/**
 * Frames the preview put on screen (a socket frame drawn, or the native layer's `shown`), for work that waits until the
 * picture has caught up: the canvas editor's look stays until the engine draws the title again.
 */
const waiting = new Set<{ left: number; done: () => void }>();

/** A frame reached the screen. */
export function noteFrameShown() {
  // Nothing waits nearly always: no work per frame then. A waiter done here leaves the set as it is walked (allowed).
  if (waiting.size === 0) return;
  for (const w of waiting) {
    w.left -= 1;
    if (w.left <= 0) w.done();
  }
}

/** Resolves once `count` more frames have been shown, or after `capMs` (the preview may draw none: not live, hidden). */
export function framesShown(count: number, capMs: number): Promise<void> {
  return new Promise((resolve) => {
    const w = {
      left: count,
      done: () => {
        clearTimeout(timer);
        waiting.delete(w);
        resolve();
      },
    };
    const timer = setTimeout(w.done, capMs);
    waiting.add(w);
  });
}
