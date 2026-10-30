import { formatPrecise, formatTime } from '../../../lib/time';

const STEPS = [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600];
const MIN_TICK_PX = 72;

/** The smallest ruler step (seconds) whose ticks are at least 72 px apart. */
export function tickStep(pps: number): number {
  return STEPS.find((step) => step * pps >= MIN_TICK_PX) ?? STEPS[STEPS.length - 1];
}

/** The ticks between `fromUs` and `toUs` only (the hour has thousands; the ruler draws a screenful). */
export function ticks(fromUs: number, toUs: number, pps: number): { step: number; times: number[] } {
  const step = tickStep(pps);
  const stepUs = Math.round(step * 1_000_000);
  const times: number[] = [];
  for (let t = Math.ceil(fromUs / stepUs) * stepUs; t <= toUs; t += stepUs) times.push(t);
  return { step, times };
}

/** Sub-second steps need tenths, or neighbouring labels would read the same ("0:00", "0:00"). */
export function tickLabel(us: number, step: number): string {
  return step < 1 ? formatPrecise(us) : formatTime(us);
}

/** Frames at least this far apart (px) get a grid on the ruler. */
const MIN_FRAME_PX = 8;

/**
 * The px between frames when the zoom is close enough to draw them (from 8 px per frame up to the closest, 40), else
 * null. There the finest tick (0.1 s) is 2.4 to 6 frames, which at 24 or 29.97 fps falls between frames.
 */
export function frameGridPx(pps: number, fps: number): number | null {
  const px = fps > 0 ? pps / fps : 0;
  return px >= MIN_FRAME_PX ? px : null;
}
