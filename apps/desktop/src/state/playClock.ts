/**
 * The UI's picture of the playback clock (Rust owns the clock): the timeline time `us` at performance time `atMs`,
 * running at `rate` (J/K/L shuttle: −2, −1, 1 or 2; negative backward) while `running`. The playhead, the timecode and
 * the timeline's follow read it, so playback moves them with a compositor animation, a 10 Hz text update and a timer
 * instead of a DOM write per frame. Frames from the engine re-anchor it only when it drifts by more than a frame.
 */
export interface ClockAnchor {
  us: number;
  atMs: number;
  running: boolean;
  rate: number;
}

/** A frame at 30 fps: re-anchoring for less would only chase delivery jitter. */
const DRIFT_US = 34_000;

let anchor: ClockAnchor = { us: 0, atMs: 0, running: false, rate: 1 };
const listeners = new Set<(anchor: ClockAnchor) => void>();

export function clockAnchor(): ClockAnchor {
  return anchor;
}

/** Timeline time on the clock at performance time `at` (now by default); never before the start, going backward. */
export function clockUs(at = performance.now()): number {
  return anchor.running
    ? Math.max(0, anchor.us + Math.max(0, at - anchor.atMs) * 1000 * anchor.rate)
    : anchor.us;
}

/**
 * Moves the clock (play, pause, seek, a re-anchor) and tells the playhead, the timecode and the follow. A listener
 * that moves it again (the loop going back at a range's end) has told every listener the newer place: the rest never
 * hear this older one after it.
 */
export function setClock(next: ClockAnchor) {
  anchor = next;
  for (const listener of listeners) {
    if (anchor !== next) return;
    listener(next);
  }
}

/** Holds the clock at `us` (paused, or waiting for the first frame of playback). */
export function holdClock(us: number) {
  setClock({ us, atMs: performance.now(), running: false, rate: 1 });
}

export function onClock(listener: (anchor: ClockAnchor) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/**
 * A playback frame showing `us` at performance time `atMs`, playing at `rate`: starts the clock, or re-anchors it if it
 * drifted or the speed changed.
 */
export function reportFrame(us: number, atMs: number, rate = 1): boolean {
  if (anchor.running && anchor.rate === rate && Math.abs(clockUs(atMs) - us) <= DRIFT_US) return false;
  setClock({ us, atMs, running: true, rate });
  return true;
}
