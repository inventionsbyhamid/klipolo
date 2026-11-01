import { describe, expect, it } from 'vitest';
import { fixtureSnapshot } from '../../test/fixtures';
import { FORMAT_SIZE } from '../../lib/layout';
import { cardAnchor, selectedBox, stageLayout } from './stageLayout';

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

/*
 * A card over the video goes to the stage's corner that covers the selected layer least: the right side first, at the
 * bottom for a layer whose centre is in the top half and at the top otherwise; the left only when both right corners
 * would cover more of it. The card's footprint is its width and the 60 % of the stage it may grow to.
 */
describe('cardAnchor', () => {
  // A 16:9 video filling an 800 x 450 stage; the card is 264 px wide.
  const area = { width: 800, height: 450 };
  const canvas = { width: 800, height: 450 };
  const anchor = (box: { x: number; y: number; w: number; h: number } | null) => cardAnchor(box, area, canvas, 264);

  it('a title in the top half gets the bottom right', () => {
    expect(anchor({ x: 0.2, y: 0.08, w: 0.6, h: 0.12 })).toBe('bottom-right');
  });

  it('a centred caption near the bottom gets the top right', () => {
    expect(anchor({ x: 0.2, y: 0.8, w: 0.6, h: 0.12 })).toBe('top-right');
  });

  it('keeps the right side when one right corner covers no more than a left one would', () => {
    // A layer in the bottom left: the top right covers none of it.
    expect(anchor({ x: 0, y: 0.6, w: 0.3, h: 0.3 })).toBe('top-right');
  });

  it('a layer filling the right half sends the card left, to the corner that covers it least', () => {
    expect(anchor({ x: 0.5, y: 0, w: 0.5, h: 1 })).toBe('top-left');
    // Its top half only: the bottom left covers none of it, and nor does the bottom right; the right side stays.
    expect(anchor({ x: 0.5, y: 0, w: 0.5, h: 0.4 })).toBe('bottom-right');
  });

  it('with nothing placed on the video (a clip, a sound, a transition) it is the top right', () => {
    expect(anchor(null)).toBe('top-right');
  });

  it('a 9:16 video centred in a wide stage: corners are the stage’s, past the video’s sides', () => {
    // 253 px wide in 800: a layer filling its right half reaches x 527 of the stage, under the right corners' 536.
    expect(cardAnchor({ x: 0.5, y: 0, w: 0.5, h: 1 }, area, { width: 253, height: 450 }, 264)).toBe('top-right');
  });
});

describe('selectedBox', () => {
  const project = fixtureSnapshot().project;

  it('a layer is its layout box in the format shown; a camera its picture; a clip, a sound or nothing has none', () => {
    expect(selectedBox(project, 'title', 'landscape')).toEqual({ x: 0.2, y: 0.08, w: 0.6, h: 0.12 });
    expect(selectedBox(project, 'camera:a', 'landscape')).toEqual({ x: 0.76, y: 0.6044, w: 0.2, h: 0.3556 });
    expect(selectedBox(project, 'a', 'landscape')).toBeNull();
    expect(selectedBox(project, 'song', 'landscape')).toBeNull();
    expect(selectedBox(project, null, 'landscape')).toBeNull();
  });
});
