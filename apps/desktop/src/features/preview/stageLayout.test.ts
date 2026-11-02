import { describe, expect, it } from 'vitest';
import { FORMAT_SIZE } from '../../lib/layout';
import { fitBox } from './stageLayout';

/*
 * The video in the stage's area: the largest box of the format's shape that fits, centred by CSS. Only the area and
 * the format decide it; nothing in the stage is kept clear for a card (it shows in the left column).
 */

describe('fitBox', () => {
  it('a 9:16 video takes the area’s height', () => {
    expect(fitBox({ width: 800, height: 450 }, FORMAT_SIZE.portrait)).toEqual({ width: 253, height: 450 });
  });

  it('a 16:9 video takes the whole of a 16:9 area', () => {
    expect(fitBox({ width: 800, height: 450 }, FORMAT_SIZE.landscape)).toEqual({ width: 800, height: 450 });
  });

  it('a 1:1 video in a wide area takes its height', () => {
    expect(fitBox({ width: 800, height: 560 }, FORMAT_SIZE.square)).toEqual({ width: 560, height: 560 });
  });

  it('a 16:9 video in a tall area takes its width', () => {
    expect(fitBox({ width: 640, height: 900 }, FORMAT_SIZE.landscape)).toEqual({ width: 640, height: 360 });
  });

  it('an area not measured yet has no video', () => {
    expect(fitBox({ width: 0, height: 0 }, FORMAT_SIZE.landscape)).toEqual({ width: 0, height: 0 });
  });
});
