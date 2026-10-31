import { describe, expect, it } from 'vitest';
import type { Track } from '../bindings/Track';
import parity from './music-runs-parity.json';
import { musicRunOf, musicRuns } from './musicRuns';

interface Fixture {
  cases: { name: string; track: Track; runs: [number, number][] }[];
}
const fixture = parity as unknown as Fixture;

describe('music runs parity with the model', () => {
  it('finds the runs the model’s refit finds', () => {
    expect(fixture.cases.length).toBe(9);
    for (const c of fixture.cases) expect(musicRuns(c.track), c.name).toEqual(c.runs);
  });

  it('gives each piece its whole run, and nothing for a layer in none', () => {
    const bed = fixture.cases[0].track;
    expect(musicRunOf([bed], 'bed-3')?.map((i) => i.id)).toEqual(['bed', 'bed-2', 'bed-3', 'bed-4']);
    const effect = fixture.cases.find((c) => c.name.startsWith('a sound effect'))!.track;
    expect(musicRunOf([effect], 'whoosh')).toBeNull();
    expect(musicRunOf([effect], 'bed')?.map((i) => i.id)).toEqual(['bed']);
    expect(musicRunOf([bed], 'nope')).toBeNull();
  });
});
