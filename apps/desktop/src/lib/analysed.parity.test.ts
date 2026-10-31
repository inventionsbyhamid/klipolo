import { describe, expect, it } from 'vitest';
import parity from './analysed-parity.json';
import { analysedAssets, type AnalysedScope } from './analysed';

interface Fixture {
  cases: { name: string; project: AnalysedScope; analysed: string[] }[];
}
const fixture = parity as unknown as Fixture;

describe('what analysis is for, as the model says', () => {
  it('is the files main-track clips play, as Rust finds them', () => {
    expect(fixture.cases.length).toBe(3);
    for (const c of fixture.cases) expect(analysedAssets(c.project).map((a) => a.id), c.name).toEqual(c.analysed);
  });

  it('gives the same array while the main track, assets and recordings stay the same', () => {
    const { project } = fixture.cases[0];
    expect(analysedAssets(project)).toBe(analysedAssets({ ...project }));
    expect(analysedAssets({ ...project, assets: [...project.assets] })).not.toBe(analysedAssets(project));
  });
});
