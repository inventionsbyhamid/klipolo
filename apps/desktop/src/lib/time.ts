export const US_PER_SEC = 1_000_000;

export function formatTime(us: number): string {
  const total = Math.max(0, Math.floor(us / US_PER_SEC));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
}

/** A length rounded to the nearest second (a 59.6 s clip is 1:00); playhead positions use formatTime. */
export function formatDuration(us: number): string {
  return formatTime(Math.round(us / US_PER_SEC) * US_PER_SEC);
}

/** Time with tenths of a second, e.g. 1:05.3. */
export function formatPrecise(us: number): string {
  const tenths = Math.floor(Math.max(0, us) / 100_000) % 10;
  return `${formatTime(us)}.${tenths}`;
}

export function frameUs(fps: number): number {
  return Math.round(US_PER_SEC / fps);
}

/**
 * Start of frame `n` at `fps`, rounded to the nearest µs: the engine's `frame_start_us` (frame 1 at 30 fps starts at
 * 33 333, frame 2 at 66 667). `fps` may be fractional (30000/1001).
 */
export function frameStartUs(n: number, fps: number): number {
  return Math.round((Math.max(0, n) * US_PER_SEC) / fps);
}

/** The frame showing at `us`: the last one whose (rounded) start is at or before it, as the engine's `frame_index`. */
export function frameIndexAt(us: number, fps: number): number {
  const t = Math.max(0, us);
  // Starts are rounded to whole µs, so the estimate can be a frame off either way.
  let n = Math.floor((t * fps) / US_PER_SEC);
  while (n > 0 && frameStartUs(n, fps) > t) n--;
  while (frameStartUs(n + 1, fps) <= t) n++;
  return n;
}

/**
 * The start of the frame `frames` away from the one showing at `us` (negative: back), within the video's frames.
 * Steps by frame index on the engine's grid, so a one-frame step always shows the next frame, at any rate and from any
 * playhead.
 */
export function stepFrames(us: number, frames: number, fps: number, durationUs: number): number {
  const lastUs = lastFrameUs(durationUs, fps);
  const last = frameIndexAt(lastUs, fps);
  const at = frameIndexAt(Math.min(us, lastUs), fps);
  return frameStartUs(Math.min(Math.max(at + frames, 0), last), fps);
}

export function snapToFrame(us: number, fps: number): number {
  const frame = Math.round((us * fps) / US_PER_SEC);
  return Math.round((frame * US_PER_SEC) / fps);
}

/** Start of the last whole frame before `durationUs` (0 for an empty video): where playback comes to rest. */
export function lastFrameUs(durationUs: number, fps: number): number {
  const frames = Math.ceil((durationUs * fps) / US_PER_SEC);
  return Math.max(0, Math.round(((frames - 1) * US_PER_SEC) / fps));
}
