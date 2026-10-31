import { describe, expect, it } from 'vitest';
import { fixtureProject } from '../../../test/fixtures';
import { snapEdges, snapTargets } from './snap';

describe('snapping', () => {
  it('collects the start, the end, the cuts, other layers and the playhead', () => {
    const t = Array.from(snapTargets(fixtureProject(), 2_100_000, new Set(['title'])));
    // 0, the cut at 20 s, the end at 30 s, the playhead, the song (0 to 30 s); never the dragged title's own edges.
    expect(t).toEqual(expect.arrayContaining([0, 2_100_000, 20_000_000, 30_000_000]));
    expect(t).not.toContain(1_000_000);
  });

  it('moves by the nearest edge within the threshold, else not at all', () => {
    const targets = Float64Array.from([0, 2_100_000, 20_000_000]);
    expect(snapEdges([2_000_000, 5_000_000], targets, 200_000)).toEqual({
      deltaUs: 100_000,
      atUs: 2_100_000,
    });
    expect(snapEdges([3_000_000, 6_000_000], targets, 200_000)).toBeNull();
  });
});
