import { describe, expect, it } from 'vitest';
import { fixtureProject } from '../../../test/fixtures';
import { nudgeOps, trimToPlayheadOps } from './keyOps';

describe('keyboard edits', () => {
  const p = fixtureProject();
  it("Q trims the selected clip's start to the playhead (the model ripples)", () => {
    expect(trimToPlayheadOps(p, 'b', 22_000_000, 'start', 30)).toEqual([
      { type: 'trimClip', id: 'b', edge: 'start', sourceUs: 2_000_000 },
    ]);
  });
  it("W trims a layer's end to the playhead, nothing when the playhead is outside it", () => {
    expect(trimToPlayheadOps(p, 'title', 2_000_000, 'end', 30)).toEqual([
      { type: 'trimItem', id: 'title', edge: 'end', atUs: 2_000_000 },
    ]);
    expect(trimToPlayheadOps(p, 'title', 10_000_000, 'end', 30)).toEqual([]);
  });
  it('nudges a layer by frames on the grid and a clip by places', () => {
    expect(nudgeOps(p, 'title', 3, 30)).toEqual([
      { type: 'moveItem', id: 'title', trackId: null, startUs: 1_100_000 },
    ]);
    expect(nudgeOps(p, 'b', -1, 30)).toEqual([{ type: 'moveClip', id: 'b', index: 0 }]);
    expect(nudgeOps(p, 'a', -1, 30)).toEqual([]);
  });
});
