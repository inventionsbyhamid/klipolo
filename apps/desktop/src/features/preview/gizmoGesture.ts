import type { BoxN } from '../../bindings/BoxN';
import {
  fontSizeFor,
  keepVisible,
  moveRect,
  resizeRect,
  rotationFor,
  sameRect,
  scaleAbout,
  snapRect,
  type Guide,
  type Handle,
  type Point,
  type Size,
  type SnapTargets,
} from '../../lib/geometry';

export type GizmoMode = 'move' | 'rotate' | { resize: Handle };

/** Where a layer is: its layout box, what is drawn of it, its turn and (text) its font size. */
export interface Placement {
  rect: BoxN;
  box: BoxN;
  rotation: number;
  fontSize: number | null;
}

/** Moves snap within this many CSS px. */
export const SNAP_PX = 6;

/** A press on a layer or one of its handles, and what the drag starts from. */
export interface Gesture {
  mode: GizmoMode;
  start: Placement;
  /** Images and videos are drawn whole in their own shape, so their corners always keep it. */
  picture: boolean;
  /** The press, in client px. */
  from: Point;
  /** The drawn box's centre in client px: what a turn pivots on. */
  centre: Point;
  /** What a move snaps to; null when nothing should. */
  snap: SnapTargets | null;
}

const NO_GUIDES: Guide[] = [];

export function samePlacement(a: Placement, b: Placement): boolean {
  return sameRect(a.rect, b.rect) && a.rotation === b.rotation && a.fontSize === b.fontSize;
}

/**
 * Where the gesture puts the layer with the pointer at `to` (client px), and the snap lines it sits on. Moves and
 * resizes work on what is drawn (so a picture's visible corner follows the pointer, and a tenth of the turned layer
 * stays on the canvas) and carry the layout box along by the same move or scale.
 */
export function placeAt(
  g: Gesture,
  to: Point,
  keys: { shift: boolean; meta: boolean },
  canvas: Size,
): { at: Placement; guides: Guide[] } {
  const s = g.start;
  if (g.mode === 'rotate') {
    // Turning a wide layer near an edge can swing it off the canvas: it slides back in.
    const rotation = rotationFor(g.centre, to, g.from, s.rotation, keys.shift);
    return { at: { ...s, ...keepVisible(s.rect, s.box, rotation, canvas), rotation }, guides: NO_GUIDES };
  }
  const dPx = { x: to.x - g.from.x, y: to.y - g.from.y };
  if (g.mode === 'move') {
    const dx = dPx.x / canvas.width;
    const dy = dPx.y / canvas.height;
    const moved = keepVisible(moveRect(s.rect, dx, dy), moveRect(s.box, dx, dy), s.rotation, canvas);
    const plain = { at: { ...s, ...moved }, guides: NO_GUIDES };
    if (!g.snap || keys.meta) return plain;
    const snapped = snapRect(moved.box, g.snap, { x: SNAP_PX / canvas.width, y: SNAP_PX / canvas.height });
    if (!snapped.guides.length) return plain;
    const rect = moveRect(moved.rect, snapped.rect.x - moved.box.x, snapped.rect.y - moved.box.y);
    // A snap that would hide the layer (its edge onto the canvas's far side) doesn't count.
    const kept = keepVisible(rect, snapped.rect, s.rotation, canvas);
    if (kept.box !== snapped.rect) return plain;
    return { at: { ...s, rect, box: snapped.rect }, guides: snapped.guides };
  }
  const handle = g.mode.resize;
  const drawn = resizeRect(s.box, handle, dPx, s.rotation, canvas, keys.shift && !g.picture);
  const centre = { x: drawn.x + drawn.w / 2, y: drawn.y + drawn.h / 2 };
  // A picture's box scales with the picture. Text drawn taller than its box (overflowing lines) keeps that overflow:
  // its box scales by the same height ratio, so the font follows smoothly.
  const h = s.rect.h * (drawn.h / s.box.h);
  const rect = g.picture
    ? scaleAbout(s.rect, drawn.w / s.box.w, centre)
    : { x: drawn.x, y: centre.y - h / 2, w: drawn.w, h };
  const kept = keepVisible(rect, drawn, s.rotation, canvas);
  // Corners, and the top and bottom edges, scale a text's font with its box; the side edges change its width only.
  const corner = handle.length === 2 || handle === 'n' || handle === 's';
  const fontSize = s.fontSize !== null && corner ? fontSizeFor(s.box, kept.box, s.fontSize) : s.fontSize;
  return { at: { ...kept, rotation: s.rotation, fontSize }, guides: NO_GUIDES };
}
