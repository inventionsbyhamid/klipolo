import { describe, expect, it } from 'vitest';
import { fixtureProject } from '../../../test/fixtures';
import { editPoints, nextEdit, previousEdit } from './editPoints';

describe('edit points', () => {
  const points = editPoints(fixtureProject());
  it('are the start, cuts, layer edges and the end, once each', () => {
    expect(Array.from(points)).toEqual([0, 1_000_000, 4_000_000, 20_000_000, 30_000_000]);
  });
  it('step from the playhead', () => {
    expect(nextEdit(points, 1_000_000)).toBe(4_000_000);
    expect(previousEdit(points, 1_000_000)).toBe(0);
    expect(nextEdit(points, 30_000_000)).toBeNull();
  });
});
