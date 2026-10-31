/** The closest zoom: 40 px per frame. */
export const MAX_PX_PER_FRAME = 40;
const FLOOR = 0.02;

/** The whole project in `viewPx` (minus a 24 px margin): the farthest zoom out. */
export function fitPps(durationUs: number, viewPx: number): number {
  if (durationUs <= 0) return 40;
  return Math.max(FLOOR, ((viewPx - 24) * 1_000_000) / durationUs);
}

export function zoomLimits(durationUs: number, viewPx: number, fps: number): [number, number] {
  return [Math.min(8, fitPps(durationUs, viewPx)), MAX_PX_PER_FRAME * fps];
}

/** The scroll position that keeps `anchorUs` at `anchorX` px into the lanes at `pps`. */
export function scrollForAnchor(anchorUs: number, anchorX: number, pps: number): number {
  return Math.max(0, (anchorUs / 1_000_000) * pps - anchorX);
}
