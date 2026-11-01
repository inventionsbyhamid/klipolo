import { describe, expect, it } from 'vitest';
import type { Key } from '../bindings/Key';
import type { Layout } from '../bindings/Layout';
import type { MotionKeys } from '../bindings/MotionKeys';
import { layoutPatchFor } from './geometry';
import { opacityWithKeys, placeWithKeys, shownPlace } from './keyedPlace';

const S = 1_000_000;
const key = (t: number, v: number): Key => ({ tUs: t * S, v });

/** A box at (0.2, 0.2), 0.4 by 0.2, with `keys`. */
function layout(keys?: Partial<MotionKeys>): Layout {
  return {
    anchor: 'center',
    rect: { x: 0.2, y: 0.2, w: 0.4, h: 0.2 },
    rotation: 0,
    opacity: 0.8,
    radius: 0,
    overrides: {},
    ...(keys ? { motion: { keys: { x: [], y: [], scale: [], rotation: [], opacity: [], ...keys } } } : {}),
  };
}

/** What a gesture does: puts the drawn box at `rect`, turned `deg`. */
const put = (rect: Layout['rect'], deg?: number) => (l: Layout) => ({
  ...layoutPatchFor(l, rect, 'landscape', 'landscape'),
  ...(deg === undefined ? {} : { rotation: deg }),
});
const place = (now: Layout, gesture: (l: Layout) => Layout, tUs = S) =>
  placeWithKeys(now, gesture, tUs, 4 * S, 'landscape', 'landscape', 1_000);

describe('a gesture on a layer with motion keys', () => {
  it('is drawn where its keys put it at the playhead', () => {
    const now = layout({ x: [key(0, 0), key(2, 0.2)], scale: [key(0, 2)] });
    const shown = shownPlace(now, S, 4 * S, 'landscape', 'landscape').rect;
    // Moved 0.1 right, twice its size about its centre (0.4, 0.3).
    expect(shown.x).toBeCloseTo(0.1);
    expect(shown.y).toBeCloseTo(0.1);
    expect(shown.w).toBeCloseTo(0.8);
    expect(shown.h).toBeCloseTo(0.4);
  });

  it('a move sets the position keys at the playhead and leaves the layer where it rests', () => {
    const now = layout({ x: [key(0, 0), key(2, 0.2)] });
    // Drawn at x 0.3 (0.1 in); the drag puts it at 0.35.
    const next = place(now, put({ x: 0.35, y: 0.25, w: 0.4, h: 0.2 }));
    expect(next.rect).toEqual(now.rect);
    expect(next.motion?.keys?.x).toEqual([key(0, 0), { tUs: S, v: expect.closeTo(0.15, 9) }, key(2, 0.2)]);
    expect(next.motion?.keys?.y.map((k) => [k.tUs, Math.round(k.v * 100) / 100])).toEqual([[S, 0.05]]);
  });

  it('a resize sets the scale key, and a turn the rotation key; without keys they change the layer', () => {
    const now = layout({ scale: [key(0, 1), key(2, 2)], rotation: [key(0, 10)] });
    // At 1 s it is drawn 1.5 times its size: 0.6 by 0.3 about (0.4, 0.3); made 0.9 wide, about the same centre.
    const grown = place(now, put({ x: -0.05, y: 0.075, w: 0.9, h: 0.45 }));
    expect(grown.rect).toEqual(now.rect);
    expect(grown.motion?.keys?.scale).toEqual([key(0, 1), { tUs: S, v: expect.closeTo(2.25, 6) }, key(2, 2)]);
    const turned = place(now, put({ x: 0.1, y: 0.15, w: 0.6, h: 0.3 }, 40));
    expect(turned.rotation).toBe(0);
    expect(turned.motion?.keys?.rotation).toEqual([key(0, 10), { tUs: S, v: 40 }]);
    // No keys: the box itself moves, as before.
    const plain = layout();
    expect(place(plain, put({ x: 0.3, y: 0.2, w: 0.4, h: 0.2 })).rect).toEqual({ x: 0.3, y: 0.2, w: 0.4, h: 0.2 });
  });

  it('an unkeyed property still changes the layer under the keys of another', () => {
    // Position keyed (0.1 right at 1 s); a turn changes the layer's own rotation.
    const now = layout({ x: [key(0, 0), key(2, 0.2)] });
    const next = place(now, put({ x: 0.3, y: 0.2, w: 0.4, h: 0.2 }, 15));
    expect(next.rotation).toBe(15);
    expect(next.motion?.keys?.rotation ?? []).toEqual([]);
  });

  it('opacity with keys sets the key at the playhead, as a share of the layer’s own', () => {
    const keyed = opacityWithKeys(layout({ opacity: [key(0, 1)] }), 0.4, S, 4 * S, 1_000);
    expect(keyed.opacity).toBe(0.8);
    expect(keyed.motion?.keys?.opacity).toEqual([key(0, 1), key(1, 0.5)]);
    expect(opacityWithKeys(layout(), 0.4, S, 4 * S, 1_000).opacity).toBe(0.4);
  });
});
