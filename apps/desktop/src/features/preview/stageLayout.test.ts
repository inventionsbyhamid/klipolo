import { describe, expect, it } from 'vitest';
import { FORMAT_SIZE } from '../../lib/layout';
import { stageLayout } from './stageLayout';

/*
 * Where the video and the card go in the stage's area. Only the area, the format and the card's slot decide it, never
 * the selection: the card shows beside the video in a slot of its own while that keeps the video large, and floats over
 * its right side otherwise.
 */

/** The card's 264 px and the 16 px gap before it. */
const SLOT = 280;

describe('stageLayout', () => {
  it('a 9:16 video with room beside it stays centred, the slot in the room at its right', () => {
    // 450 px tall: the video is 253 px wide, leaving 1047 px either side of it in all.
    expect(stageLayout({ width: 1300, height: 450 }, FORMAT_SIZE.portrait, SLOT)).toEqual({
      card: 'slot',
      pad: 0,
      canvas: { width: 253, height: 450 },
    });
  });

  it('a 9:16 video with less room moves left only as far as the slot needs', () => {
    // Centred in 800 px its right edge would be at 527; the slot starts at 520.
    expect(stageLayout({ width: 800, height: 450 }, FORMAT_SIZE.portrait, SLOT)).toEqual({
      card: 'slot',
      pad: 13,
      canvas: { width: 253, height: 450 },
    });
  });

  it('a 1:1 video that keeps most of its size beside the slot gets the slot', () => {
    // 520 px wide beside the slot, of 560: it keeps 93 % of its size.
    expect(stageLayout({ width: 800, height: 560 }, FORMAT_SIZE.square, SLOT)).toEqual({
      card: 'slot',
      pad: SLOT,
      canvas: { width: 520, height: 520 },
    });
  });

  it('a 16:9 video that would lose more than a fifth of its size keeps it all, and the card floats over it', () => {
    expect(stageLayout({ width: 800, height: 450 }, FORMAT_SIZE.landscape, SLOT)).toEqual({
      card: 'over',
      pad: 0,
      canvas: { width: 800, height: 450 },
    });
  });

  it('a 16:9 video in a wide area gets the slot when it keeps most of its size', () => {
    // 1460 px beside the slot, of 1740: 84 %.
    expect(stageLayout({ width: 1740, height: 1060 }, FORMAT_SIZE.landscape, SLOT)).toEqual({
      card: 'slot',
      pad: SLOT,
      canvas: { width: 1460, height: 821 },
    });
  });

  it('an area not measured yet has no video and no slot', () => {
    expect(stageLayout({ width: 0, height: 0 }, FORMAT_SIZE.landscape, SLOT)).toEqual({
      card: 'over',
      pad: 0,
      canvas: { width: 0, height: 0 },
    });
  });
});
