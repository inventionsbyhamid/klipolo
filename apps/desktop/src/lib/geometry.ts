import type { BoxN } from '../bindings/BoxN';
import type { FormatKind } from '../bindings/FormatKind';
import type { Layout } from '../bindings/Layout';
import { anchorForRect } from './layout';
import type { Size } from './preview';

export type { Size };

/** Smallest layer: 2 % of the canvas's shorter side. */
export const MIN_SIZE = 0.02;
/** At least this share of a layer stays on the canvas, so a dragged layer can hang off an edge but never vanish. */
export const MIN_VISIBLE = 0.1;

export function moveRect(rect: BoxN, dx: number, dy: number): BoxN {
  return { ...rect, x: rect.x + dx, y: rect.y + dy };
}

export function clampVisible(rect: BoxN): BoxN {
  return {
    ...rect,
    x: Math.min(Math.max(rect.x, -rect.w * (1 - MIN_VISIBLE)), 1 - rect.w * MIN_VISIBLE),
    y: Math.min(Math.max(rect.y, -rect.h * (1 - MIN_VISIBLE)), 1 - rect.h * MIN_VISIBLE),
  };
}

export function sameRect(a: BoxN, b: BoxN): boolean {
  return a.x === b.x && a.y === b.y && a.w === b.w && a.h === b.h;
}

/**
 * The layout after placing a layer at `rect` in `format`. The primary format moves the layer itself (and re-anchors
 * it, as Rust's `anchor_for_rect` would); other formats, or a format that already has its own placement, get an
 * override, so a 9:16 tweak never moves the 16:9 layout.
 */
export function layoutPatchFor(layout: Layout, rect: BoxN, format: FormatKind, primary: FormatKind): Layout {
  if (format !== primary || layout.overrides[format])
    return { ...layout, overrides: { ...layout.overrides, [format]: rect } };
  return { ...layout, rect, anchor: anchorForRect(rect) };
}

/** A pointer delta in CSS pixels as canvas units. */
export function toCanvasDelta(dxPx: number, dyPx: number, canvas: Size): { dx: number; dy: number } {
  return { dx: dxPx / canvas.width, dy: dyPx / canvas.height };
}

/**
 * Keep a tenth of what's drawn on the canvas, moving the layout `rect` with it. What's drawn is `box` turned by
 * `rotation` about its centre, so the tenth is of its bounding box (worked out in `canvas`'s pixels, whose shape is
 * the format's). `box` and `rect` differ for a picture drawn inside a box of another shape.
 */
export function keepVisible(
  rect: BoxN,
  box: BoxN,
  rotation: number,
  canvas: Size,
): { rect: BoxN; box: BoxN } {
  let bounds = box;
  if (rotation % 180 !== 0) {
    const r = (rotation * Math.PI) / 180;
    const cos = Math.abs(Math.cos(r));
    const sin = Math.abs(Math.sin(r));
    const wPx = box.w * canvas.width;
    const hPx = box.h * canvas.height;
    const w = (wPx * cos + hPx * sin) / canvas.width;
    const h = (wPx * sin + hPx * cos) / canvas.height;
    bounds = { x: box.x + (box.w - w) / 2, y: box.y + (box.h - h) / 2, w, h };
  }
  const kept = clampVisible(bounds);
  const dx = kept.x - bounds.x;
  const dy = kept.y - bounds.y;
  if (dx === 0 && dy === 0) return { rect, box };
  return { rect: moveRect(rect, dx, dy), box: moveRect(box, dx, dy) };
}

/**
 * The eight handles: corners keep the shape, the top and bottom edges too (from the opposite edge), and the left and
 * right edges change a text's width.
 */
export type Handle = 'nw' | 'ne' | 'sw' | 'se' | 'n' | 's' | 'w' | 'e';
export interface Point {
  x: number;
  y: number;
}
export interface Guide {
  axis: 'x' | 'y';
  at: number;
}
export interface SnapTargets {
  x: number[];
  y: number[];
}

function rotate(p: Point, deg: number): Point {
  const r = (deg * Math.PI) / 180;
  return { x: p.x * Math.cos(r) - p.y * Math.sin(r), y: p.x * Math.sin(r) + p.y * Math.cos(r) };
}

/**
 * Resize `start` by a pointer delta (canvas px) on `handle`, keeping the opposite corner or side where it was even
 * on a rotated layer. Corners keep the shape unless `free`; side handles (text) change only the width.
 */
export function resizeRect(
  start: BoxN,
  handle: Handle,
  dPx: Point,
  rotationDeg: number,
  canvas: Size,
  free: boolean,
): BoxN {
  const W = canvas.width;
  const H = canvas.height;
  const w0 = start.w * W;
  const h0 = start.h * H;
  const centre = { x: (start.x + start.w / 2) * W, y: (start.y + start.h / 2) * H };
  const sx = handle.includes('e') ? 1 : handle.includes('w') ? -1 : 0;
  const sy = handle.includes('s') ? 1 : handle.includes('n') ? -1 : 0;
  const d = rotate(dPx, -rotationDeg);
  let w = w0 + sx * d.x;
  let h = sy === 0 ? h0 : h0 + sy * d.y;
  const min = MIN_SIZE * Math.min(W, H);
  if (sy !== 0 && sx === 0 && !free) {
    // A top or bottom edge: the height follows the pointer, the width keeps the shape about the centre line.
    const s = Math.max((h0 + sy * d.y) / h0, min / w0, min / h0);
    w = w0 * s;
    h = h0 * s;
  } else if (sy !== 0 && !free) {
    // Project the drag onto the diagonal so the corner follows the pointer smoothly; keep the shape at the minimum.
    const s = Math.max(1 + (sx * d.x * w0 + sy * d.y * h0) / (w0 * w0 + h0 * h0), min / w0, min / h0);
    w = w0 * s;
    h = h0 * s;
  }
  w = Math.max(w, min);
  h = Math.max(h, min);
  const fixed = rotate({ x: (-sx * w0) / 2, y: (-sy * h0) / 2 }, rotationDeg);
  const back = rotate({ x: (sx * w) / 2, y: (sy * h) / 2 }, rotationDeg);
  const cx = centre.x + fixed.x + back.x;
  const cy = centre.y + fixed.y + back.y;
  return { x: (cx - w / 2) / W, y: (cy - h / 2) / H, w: w / W, h: h / H };
}

/** `rect` scaled by `s` and centred on `centre`: a picture's layout box follows what's drawn of it. */
export function scaleAbout(rect: BoxN, s: number, centre: Point): BoxN {
  const w = rect.w * s;
  const h = rect.h * s;
  return { x: centre.x - w / 2, y: centre.y - h / 2, w, h };
}

/** A text layer's font follows its box height (Canva-style), within the model's sensible range. */
export function fontSizeFor(start: BoxN, next: BoxN, size: number): number {
  return Math.min(400, Math.max(8, Math.round(((size * next.h) / start.h) * 2) / 2));
}

/** Degrees clockwise from the press, snapped to 0/90/180/270 within 3°, or to 15° steps with `fine` (Shift). */
export function rotationFor(
  center: Point,
  pointer: Point,
  startPointer: Point,
  startRotation: number,
  fine: boolean,
): number {
  const angle = (p: Point) => (Math.atan2(p.y - center.y, p.x - center.x) * 180) / Math.PI;
  let deg = startRotation + angle(pointer) - angle(startPointer);
  deg = ((((deg + 180) % 360) + 360) % 360) - 180;
  if (fine) return Math.round(deg / 15) * 15;
  for (const snap of [-180, -90, 0, 90, 180])
    if (Math.abs(deg - snap) <= 3) return snap === -180 ? 180 : snap;
  return Math.round(deg * 10) / 10;
}

/** Lines a moving layer snaps to: the canvas edges and centre lines, other layers' edges and centres, the safe area. */
export function snapTargets(others: BoxN[], safe: BoxN | null): SnapTargets {
  const x = [0, 0.5, 1];
  const y = [0, 0.5, 1];
  for (const r of others) {
    x.push(r.x, r.x + r.w / 2, r.x + r.w);
    y.push(r.y, r.y + r.h / 2, r.y + r.h);
  }
  if (safe) {
    x.push(safe.x, safe.x + safe.w);
    y.push(safe.y, safe.y + safe.h);
  }
  return { x, y };
}

/** Move `rect` so its nearest edge or centre lies on a target within `threshold` (canvas units per axis). */
export function snapRect(
  rect: BoxN,
  targets: SnapTargets,
  threshold: Point,
): { rect: BoxN; guides: Guide[] } {
  const guides: Guide[] = [];
  // Runs on every pointer event of a move: plain numbers, no allocation per candidate.
  const axis = (pos: number, size: number, lines: number[], t: number, name: 'x' | 'y') => {
    let delta = Infinity;
    let at = 0;
    for (let k = 0; k < 3; k++) {
      const edge = pos + (size * k) / 2; // the near edge, the centre, the far edge
      for (const line of lines) {
        const d = line - edge;
        if (Math.abs(d) <= t && Math.abs(d) < Math.abs(delta)) {
          delta = d;
          at = line;
        }
      }
    }
    if (delta === Infinity) return pos;
    guides.push({ axis: name, at });
    return pos + delta;
  };
  const x = axis(rect.x, rect.w, targets.x, threshold.x, 'x');
  const y = axis(rect.y, rect.h, targets.y, threshold.y, 'y');
  return { rect: { ...rect, x, y }, guides };
}
