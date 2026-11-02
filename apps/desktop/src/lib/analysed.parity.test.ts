import { describe, expect, it } from 'vitest';
import parity from './analysed-parity.json';
import voicesParity from './voices-parity.json';
import { analysable, analysedAssets, voiceOf, wordsOnly, type AnalysedScope } from './analysed';

interface Fixture {
  cases: { name: string; project: AnalysedScope; analysed: string[] }[];
}
const fixture = parity as unknown as Fixture;

/** Rust's take voice files (`speech::run_spans`, by take clip) and words-only files (`analysed::words_only`). */
interface Voices {
  cases: {
    name: string;
    project: AnalysedScope;
    takes: { clip: string; recording: string; voices: string[] }[];
    wordsOnly: string[];
  }[];
}
const voices = voicesParity as unknown as Voices;

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

describe('a take’s voice, as the captions read it', () => {
  it('is the files of each take the page offers, as Rust’s runs find them', () => {
    expect(voices.cases.length).toBe(10);
    for (const c of voices.cases) {
      const heard = new Set<string>();
      for (const take of c.takes) {
        for (const file of take.voices) {
          expect(voiceOf(c.project, file), `${c.name}: ${file}`).toBe(take.recording);
          heard.add(file);
        }
      }
      const offered = analysable(c.project).filter((a) => voiceOf(c.project, a.id) !== null);
      expect(offered.map((a) => a.id).sort(), c.name).toEqual([...heard].sort());
    }
  });

  it('knows the files heard for their words only, as Rust does', () => {
    for (const c of voices.cases) {
      const heard = c.project.assets.filter((a) => wordsOnly(c.project, a.id));
      expect(heard.map((a) => a.id), c.name).toEqual(c.wordsOnly);
    }
  });
});
