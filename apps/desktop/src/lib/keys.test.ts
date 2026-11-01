import { describe, expect, it } from 'vitest';
import type { Ease } from '../bindings/Ease';
import type { Key } from '../bindings/Key';
import {
  MAX_KEYS,
  SPRING,
  easeAt,
  levelAt,
  motionKeyTimes,
  movedKey,
  sampleMotion,
  valueAt,
  withKey,
  withMotionKeys,
  withoutKey,
} from './keys';

const S = 1_000_000;
const key = (t: number, v: number, ease?: Ease): Key => (ease ? { tUs: t * S, v, ease } : { tUs: t * S, v });

describe('keys set by hand', () => {
  it('every ease starts at 0 and ends at 1, and the curves bend where they say', () => {
    const eases: Ease[] = [
      'linear',
      'ease',
      'easeIn',
      'easeOut',
      'easeInOut',
      'power2InOut',
      'backOut',
      'elasticOut',
      'bounceOut',
      'sineIn',
      'expoInOut',
      'circOut',
      SPRING,
      { cubicBezier: { x1: 0.4, y1: 0, x2: 0.2, y2: 1 } },
      { steps: { n: 4 } },
    ];
    for (const ease of eases) {
      expect(easeAt(ease, 0)).toBeCloseTo(0, 9);
      expect(easeAt(ease, 1)).toBeCloseTo(1, 9);
    }
    expect(easeAt('easeIn', 0.5)).toBeLessThan(0.5);
    expect(easeAt('easeOut', 0.5)).toBeGreaterThan(0.5);
    expect(easeAt('easeInOut', 0.5)).toBeCloseTo(0.5, 6);
    expect(easeAt('backOut', 0.8)).toBeGreaterThan(1);
    // The card's spring overshoots a little before it settles.
    const peak = Math.max(...Array.from({ length: 101 }, (_, i) => easeAt(SPRING, i / 100)));
    expect(peak).toBeGreaterThan(1);
    expect(peak).toBeLessThan(1.3);
    expect(easeAt('hold', 0.99)).toBe(0);
    expect(easeAt({ steps: { n: 4 } }, 0.3)).toBe(0.25);
  });

  it('a value holds the first key before it and the last after it, eased in between', () => {
    const keys = [key(1, 0), key(3, 0.2, 'easeInOut')];
    expect(valueAt(keys, 0, 5)).toBe(0);
    expect(valueAt(keys, 2 * S, 5)).toBeCloseTo(0.1, 6);
    expect(valueAt(keys, 9 * S, 5)).toBe(0.2);
    expect(valueAt([], S, 5)).toBe(5);
    // A level stays within 0 to 2 when an ease overshoots, and is the volume without keys.
    expect(levelAt([key(0, 2), key(1, 1.9, 'backOut')], 0.8 * S, 1)).toBeLessThanOrEqual(2);
    expect(levelAt(undefined, S, 0.4)).toBe(0.4);
  });

  it('adds a key in order, replaces the one at the playhead, and stops at 64', () => {
    const keys = [key(1, 0), key(3, 1)];
    expect(withKey(keys, 2 * S, 0.5)).toEqual([key(1, 0), key(2, 0.5), key(3, 1)]);
    // Within the slack of a key: that key, at its own time, keeping its ease.
    expect(withKey([key(1, 0), key(3, 1, 'easeIn')], 3 * S + 10, 0.4, 100)).toEqual([key(1, 0), key(3, 0.4, 'easeIn')]);
    const full = Array.from({ length: MAX_KEYS }, (_, i) => key(i, 1));
    expect(withKey(full, 100 * S, 1)).toBeNull();
    expect(withoutKey(keys, 3 * S + 10, 100)).toEqual([key(1, 0)]);
  });

  it('moves a key within the layer, never onto another, and copies it with Option', () => {
    const keys = [key(1, 0), key(3, 1)];
    expect(movedKey(keys, S, 2 * S, 5 * S)).toEqual([key(2, 0), key(3, 1)]);
    // Onto its neighbour: 1 µs short of it.
    expect(movedKey(keys, S, 3 * S, 5 * S).map((k) => k.tUs)).toEqual([3 * S - 1, 3 * S]);
    expect(movedKey(keys, 3 * S, 9 * S, 5 * S).map((k) => k.tUs)).toEqual([S, 5 * S]);
    expect(movedKey(keys, S, 2 * S, 5 * S, true)).toEqual([key(1, 0), key(2, 0), key(3, 1)]);
  });

  it('a layer keeps its enter and exit when its keys go, and loses its motion when nothing is left', () => {
    const fade = { preset: 'klipolo.builtin/motion.fade', params: {}, durationUs: 300_000 };
    const keyed = withMotionKeys({ enter: fade }, () => ({ x: [key(0, 0.1)] }));
    expect(keyed?.keys?.x).toEqual([key(0, 0.1)]);
    expect(motionKeyTimes(keyed)).toEqual([0]);
    expect(withMotionKeys(keyed, () => ({ x: [] }))).toEqual({ enter: fade });
    expect(withMotionKeys({ keys: { x: [key(0, 1)], y: [], scale: [], rotation: [], opacity: [] } }, () => ({ x: [] }))).toBeUndefined();
    expect(sampleMotion(keyed, S, 2 * S)).toEqual({ dx: 0.1, dy: 0, scale: 1, rotation: 0, opacity: 1 });
  });
});
