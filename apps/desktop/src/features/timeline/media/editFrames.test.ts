import { describe, expect, it } from 'vitest';
import { fixtureProject } from '../../../test/fixtures';
import { editPair } from './editFrames';

const S = 1_000_000;

/** "a" plays the recording 2 to 20 s, "b" broll 2 to 8 s, "c" the recording 30 to 40 s. */
function abc() {
  const p = fixtureProject();
  const [a, b] = p.timeline.main;
  p.timeline.main = [
    { ...a, sourceInUs: 2 * S },
    { ...b, sourceInUs: 2 * S, sourceOutUs: 8 * S },
    { ...a, id: 'c', sourceInUs: 30 * S, sourceOutUs: 40 * S },
  ];
  return p;
}

describe('the two frames that meet at an edit', () => {
  it("a roll's: the clip before's new last frame and the clip after's new first, from their files", () => {
    expect(editPair(abc(), 'roll', 0, S)).toEqual([
      { path: '/media/screen', fileUs: 21 * S - 33_333 },
      { path: '/media/broll', fileUs: 3 * S },
    ]);
  });

  it("a slip's: its own new first and last frames; a slide's: its neighbours' at its new ends", () => {
    expect(editPair(abc(), 'slip', 1, -S)).toEqual([
      { path: '/media/broll', fileUs: S },
      { path: '/media/broll', fileUs: 7 * S - 33_333 },
    ]);
    expect(editPair(abc(), 'slide', 1, -S)).toEqual([
      { path: '/media/screen', fileUs: 19 * S - 33_333 },
      { path: '/media/screen', fileUs: 29 * S },
    ]);
    expect(editPair(abc(), 'slide', 0, S)).toBeNull();
  });
});
