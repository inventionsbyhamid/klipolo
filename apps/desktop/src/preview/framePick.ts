import type { FrameMeta } from './frameTrailer';

/** One display refresh at 60 Hz (WKWebView's requestAnimationFrame stays at 60 Hz even on ProMotion). */
export const REFRESH_MS = 1000 / 60;
/** Frames kept waiting for their due time (playback renders about one frame ahead). */
const KEEP = 3;

export interface Queued {
  buf: ArrayBuffer;
  meta: FrameMeta;
}

/**
 * Frames from the socket, drawn by due time in `requestAnimationFrame` rather than on arrival (arrival jitter against
 * 16.7 ms refreshes turns 2:2 cadence into 1:3 judder). Mirrors `klipolo_engine::present::pick`.
 */
export class FrameQueue {
  private frames: Queued[] = [];

  push(frame: Queued) {
    this.frames.push(frame);
    if (this.frames.length > KEEP) this.frames.shift();
  }

  /**
   * The newest frame due by the refresh at `targetMs` (epoch ms), within half a refresh; it and every older frame
   * leave the queue. Stills (due 0) are always due. Null: keep what is on screen.
   */
  take(targetMs: number): Queued | null {
    const by = targetMs + REFRESH_MS / 2;
    let chosen = -1;
    for (let i = 0; i < this.frames.length; i++) if (this.frames[i].meta.dueMs <= by) chosen = i;
    if (chosen < 0) return null;
    const frame = this.frames[chosen];
    this.frames.splice(0, chosen + 1);
    return frame;
  }

  get size() {
    return this.frames.length;
  }

  clear() {
    this.frames.length = 0;
  }
}
