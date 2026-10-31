import { describe, expect, it } from 'vitest';
import { frameGridPx, tickLabel, tickStep, ticks } from './ticks';

describe('ruler ticks', () => {
  it('picks the smallest step at least 72 px apart, down to a tenth of a second', () => {
    expect(tickStep(40)).toBe(2);
    expect(tickStep(1200)).toBe(0.1);
    expect(tickStep(0.3)).toBe(300);
  });

  it('makes only the ticks in the window', () => {
    const { step, times } = ticks(55_000_000, 65_000_000, 40);
    expect(step).toBe(2);
    expect(times).toEqual([56e6, 58e6, 60e6, 62e6, 64e6]);
  });

  it('labels sub-second steps with tenths', () => {
    expect(tickLabel(500_000, 0.5)).toBe('0:00.5');
    expect(tickLabel(3_600_000_000, 600)).toBe('1:00:00');
  });

  it('draws a frame grid only at the closest zooms', () => {
    expect(frameGridPx(1200, 30)).toBe(40);
    expect(frameGridPx(240, 30)).toBe(8);
    expect(frameGridPx(200, 30)).toBeNull();
    expect(frameGridPx(40, 0)).toBeNull();
  });
});
