/** One timeline's zoom gestures: what waits for them to rest. */
export interface ZoomRest {
  /** Runs `begin` now, or once the zoom rests if one is under way; returns what cancels a start still waiting. */
  whenRested(pps: number, begin: () => void): () => void;
}

/**
 * A pinch zooms once a frame. Until the zoom has rested for `restMs`, blocks keep their picture (stretched with the
 * ruler, `anchorCanvas`) and load and draw nothing; then each draws once, instead of every block every frame.
 */
export function createZoomRest(restMs = 100): ZoomRest {
  let lastPps = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const waiting = new Set<() => void>();
  return {
    whenRested(pps, begin) {
      if (lastPps !== 0 && pps !== lastPps) {
        clearTimeout(timer);
        timer = setTimeout(() => {
          timer = undefined;
          const starts = [...waiting];
          waiting.clear();
          for (const start of starts) start();
        }, restMs);
      }
      lastPps = pps;
      if (timer === undefined) begin();
      else waiting.add(begin);
      return () => waiting.delete(begin);
    },
  };
}
