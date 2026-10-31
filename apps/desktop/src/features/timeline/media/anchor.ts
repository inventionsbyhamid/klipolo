import { usToPx } from '../scale';

/** How a block is on screen: its zoom, and where its source starts and how fast it plays. */
export interface BlockView {
  pps: number;
  sourceInUs: number;
  speed: number;
}

/**
 * What a canvas holds: from `offUs` after its block's start (source time `srcUs` there), drawn at `pps` and `speed`.
 */
export interface Drawn {
  offUs: number;
  srcUs: number;
  pps: number;
  speed: number;
}
const drawnParts = new WeakMap<HTMLCanvasElement, Drawn>();
const views = new WeakMap<HTMLCanvasElement, BlockView>();

/**
 * The transform (from the canvas's left edge, which is where `drawn` put it) that keeps what it drew over the same
 * source time as its block is now: its start moved to where that source time is, and stretched with the zoom (and a
 * speed change). None when that is where it was drawn.
 */
export function anchorTransform(drawn: Drawn, view: BlockView): string {
  const dx = usToPx((drawn.srcUs - view.sourceInUs) / view.speed, view.pps) - usToPx(drawn.offUs, drawn.pps);
  const k = ((view.pps / drawn.pps) * drawn.speed) / view.speed;
  return Math.abs(dx) < 1e-6 && k === 1 ? '' : `translateX(${dx}px) scaleX(${k})`;
}

/**
 * Keeps `canvas` over the source time it drew while its block is on screen as `view` (as the block renders): a window
 * step leaves the picture where it is until the new part replaces it, a pinch stretches it with the ruler until the
 * zoom rests and it redraws, and a trimmed start doesn't drag the picture along.
 */
export function anchorCanvas(canvas: HTMLCanvasElement, view: BlockView) {
  views.set(canvas, view);
  const drawn = drawnParts.get(canvas);
  canvas.style.transform = drawn ? anchorTransform(drawn, view) : '';
}

/** `canvas` now shows `drawn` (as it draws): kept over that source time for the view its block is in now. */
export function placeDrawn(canvas: HTMLCanvasElement, drawn: Drawn) {
  drawnParts.set(canvas, drawn);
  const view = views.get(canvas);
  canvas.style.transform = view ? anchorTransform(drawn, view) : '';
}

/** The zoom `canvas`'s block is on screen at now (undefined before it first renders). */
export function viewPps(canvas: HTMLCanvasElement): number | undefined {
  return views.get(canvas)?.pps;
}
