import { describe, expect, it } from 'vitest';
import type { Layout } from '../bindings/Layout';
import {
  clampVisible,
  fontSizeFor,
  layoutPatchFor,
  moveRect,
  resizeRect,
  rotationFor,
  snapRect,
  snapTargets,
} from './geometry';
import { anchorForRect } from './layout';
import { safeRect } from './safeAreas';

const layout: Layout = {
  anchor: 'topCenter',
  rect: { x: 0.2, y: 0.08, w: 0.6, h: 0.12 },
  rotation: 0,
  opacity: 1,
  radius: 0,
  overrides: {},
};

describe('gizmo geometry', () => {
  it('moves and keeps a tenth of the layer on the canvas', () => {
    const moved = moveRect(layout.rect, 0.1, 0.05);
    expect([moved.x, moved.y, moved.w, moved.h].map((v) => +v.toFixed(9))).toEqual([0.3, 0.13, 0.6, 0.12]);
    const kept = clampVisible({ x: 1.5, y: -2, w: 0.2, h: 0.2 });
    expect(kept.x).toBeCloseTo(0.98);
    expect(kept.y).toBeCloseTo(-0.18);
  });

  it('writes the rect in the primary format and an override elsewhere', () => {
    const rect = { x: 0.6, y: 0.7, w: 0.3, h: 0.2 };
    expect(layoutPatchFor(layout, rect, 'landscape', 'landscape')).toEqual({
      ...layout,
      rect,
      anchor: 'bottomRight',
    });
    expect(layoutPatchFor(layout, rect, 'portrait', 'landscape')).toEqual({
      ...layout,
      overrides: { portrait: rect },
    });
  });

  it('anchors to the nearest of nine regions', () => {
    expect(anchorForRect({ x: 0.4, y: 0.4, w: 0.2, h: 0.2 })).toBe('center');
    expect(anchorForRect({ x: 0, y: 0, w: 0.1, h: 0.1 })).toBe('topLeft');
  });
});

const canvas = { width: 1000, height: 1000 };

describe('resize, rotate and snap', () => {
  it('corner resize keeps the shape unless freed', () => {
    const start = { x: 0.1, y: 0.1, w: 0.2, h: 0.2 };
    expect(resizeRect(start, 'se', { x: 100, y: 50 }, 0, canvas, false)).toEqual({
      x: 0.1,
      y: 0.1,
      w: 0.275,
      h: 0.275,
    });
    expect(resizeRect(start, 'se', { x: 100, y: 50 }, 0, canvas, true)).toEqual({
      x: 0.1,
      y: 0.1,
      w: 0.3,
      h: 0.25,
    });
  });

  it('keeps the opposite side fixed on a rotated layer', () => {
    const r = resizeRect({ x: 0.4, y: 0.4, w: 0.2, h: 0.1 }, 'e', { x: 0, y: 50 }, 90, canvas, false);
    expect(r.x).toBeCloseTo(0.375);
    expect(r.y).toBeCloseTo(0.425);
    expect(r.w).toBeCloseTo(0.25);
    expect(r.h).toBeCloseTo(0.1);
  });

  it('never shrinks below 2 % and scales text with its height', () => {
    expect(
      resizeRect({ x: 0.1, y: 0.1, w: 0.2, h: 0.2 }, 'se', { x: -500, y: -500 }, 0, canvas, false).w,
    ).toBeCloseTo(0.02);
    expect(fontSizeFor({ x: 0, y: 0, w: 0.4, h: 0.1 }, { x: 0, y: 0, w: 0.6, h: 0.15 }, 72)).toBe(108);
    expect(fontSizeFor({ x: 0, y: 0, w: 0.4, h: 0.1 }, { x: 0, y: 0, w: 0.04, h: 0.001 }, 72)).toBe(8);
  });

  it('rotation snaps to right angles within 3°, or to 15° with Shift', () => {
    const c = { x: 0, y: 0 };
    expect(rotationFor(c, { x: 10, y: -0.3 }, { x: 0, y: -10 }, 0, false)).toBe(90);
    expect(rotationFor(c, { x: 10, y: -8.4 }, { x: 0, y: -10 }, 0, false)).toBeCloseTo(50, 0);
    expect(rotationFor(c, { x: 10, y: -8.4 }, { x: 0, y: -10 }, 0, true)).toBe(45);
  });

  it('snaps the nearest edge or centre within the threshold only', () => {
    const t = snapTargets([], null);
    const near = snapRect({ x: 0.296, y: 0.2, w: 0.4, h: 0.1 }, t, { x: 0.0075, y: 0.0075 });
    expect(near.rect.x).toBeCloseTo(0.3);
    expect(near.guides).toEqual([{ axis: 'x', at: 0.5 }]);
    expect(snapRect({ x: 0.28, y: 0.2, w: 0.4, h: 0.1 }, t, { x: 0.0075, y: 0.0075 }).guides).toEqual([]);
    expect(snapTargets([], safeRect('portrait', 'all')).y).toContain(0.14);
    expect(safeRect('landscape', 'all')).toBeNull();
  });
});
