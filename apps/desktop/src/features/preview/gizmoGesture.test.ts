import { describe, expect, it } from 'vitest';
import type { BoxN } from '../../bindings/BoxN';
import { placeAt, type Gesture } from './gizmoGesture';

/*
 * A side handle on a picture: the picture keeps its shape, so dragging its right (or left) edge scales it about the
 * drawn box's centre, its height following its width; on text the same handle changes the width alone.
 */

const canvas = { width: 1920, height: 1080 };
const box: BoxN = { x: 0.4, y: 0.4, w: 0.2, h: 0.2 };
const gesture = (handle: 'e' | 'w', picture: boolean): Gesture => ({
  mode: { resize: handle },
  start: { rect: box, box, rotation: 0, fontSize: picture ? null : 72 },
  picture,
  from: { x: 0, y: 0 },
  centre: { x: 960, y: 540 },
  snap: null,
});
const keys = { shift: false, meta: false };

describe('a side handle', () => {
  it('scales a picture whole: its right edge out by a tenth of the frame makes it half as big again', () => {
    const { at } = placeAt(gesture('e', true), { x: 192, y: 0 }, keys, canvas);
    expect(at.rect.w).toBeCloseTo(0.3);
    expect(at.rect.h).toBeCloseTo(0.3);
    // About the drawn box's centre: its left edge stays, its middle height too.
    expect(at.rect.x + at.rect.w / 2).toBeCloseTo(0.55);
    expect(at.rect.y + at.rect.h / 2).toBeCloseTo(0.5);
  });

  it('on the left edge, the picture grows the other way', () => {
    const { at } = placeAt(gesture('w', true), { x: -192, y: 0 }, keys, canvas);
    expect(at.rect.w).toBeCloseTo(0.3);
    expect(at.rect.h).toBeCloseTo(0.3);
    expect(at.rect.x + at.rect.w / 2).toBeCloseTo(0.45);
  });

  it('changes a text box’s width alone, and leaves its font', () => {
    const { at } = placeAt(gesture('e', false), { x: 192, y: 0 }, keys, canvas);
    expect(at.rect.w).toBeCloseTo(0.3);
    expect(at.rect.h).toBeCloseTo(0.2);
    expect(at.fontSize).toBe(72);
  });
});
