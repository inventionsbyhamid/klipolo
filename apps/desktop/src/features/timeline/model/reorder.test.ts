import { describe, expect, it } from 'vitest';
import { longProject } from '../../../test/longProject';
import { mainIndex } from './index';
import { refitNote } from './refit';
import { insertionIndex, reorderShifts } from './reorder';

describe('reordering the main track', () => {
  const index = mainIndex(longProject().timeline.main);
  it('lands after every other clip whose middle the pointer passed', () => {
    expect(insertionIndex(index, 0, 1_000_000)).toBe(0);
    // Clip 2's middle is 6 s: just past it, clip 0 lands after clips 1 and 2.
    expect(insertionIndex(index, 0, 6_100_000)).toBe(2);
  });

  it("slides the clips in between by the dragged clip's length", () => {
    expect([...reorderShifts(index, 0, 2)]).toEqual([
      [1, -2_400_000],
      [2, -2_400_000],
    ]);
    expect([...reorderShifts(index, 3, 1)]).toEqual([
      [1, 2_400_000],
      [2, 2_400_000],
    ]);
    // Dragged a thousand clips on, only those that can be on screen slide.
    expect([...reorderShifts(index, 0, 1000, [500, 503]).keys()]).toEqual([500, 501, 502]);
  });

  it('says what an edit does to transitions as a sentence of its own, never a fragment', () => {
    expect(refitNote({ removed: 1, shortened: 0 })).toBe('A transition is removed.');
    expect(refitNote({ removed: 2, shortened: 1 })).toBe('2 transitions are removed and one is shortened.');
    expect(refitNote({ removed: 0, shortened: 3 })).toBe('3 transitions are shortened.');
    expect(refitNote({ removed: 0, shortened: 0 })).toBe('');
  });
});
