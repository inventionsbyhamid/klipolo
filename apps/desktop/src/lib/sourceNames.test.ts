import { describe, expect, it } from 'vitest';
import type { RecordingSource } from '../bindings/RecordingSource';
import { twoTakes } from '../test/sounds';
import { labelText } from './labels';
import { roleName, soundName, soundParam, sourceName } from './sourceNames';

describe("a recording's sounds by name", () => {
  it('names a source Klipolo named by its role, and keeps the name a person gave it', () => {
    const r = twoTakes().recordings[0];
    const [, mic, computer] = r.sources;
    expect(sourceName(mic)).toBe('Mic');
    expect(sourceName(computer)).toBe('Computer sound');
    expect(sourceName({ ...mic, label: 'Lav' })).toBe('Lav');
    expect(roleName('gameAudio')).toBe('Game sound');
  });

  it("numbers a take's second mic Klipolo named, never one the person named", () => {
    const r = twoTakes().recordings[0];
    const second: RecordingSource = { ...r.sources[1], id: 'mic-2' };
    r.sources.push(second);
    expect(soundName(r, second)).toBe('Mic 2');
    expect(soundName(r, r.sources[1])).toBe('Mic');
    second.label = 'Lav';
    expect(soundName(r, second)).toBe('Lav');
  });

  it("names a picture's own sound after it", () => {
    const r = twoTakes().recordings[0];
    const screen = r.sources[0];
    expect(soundName(r, screen)).toBe("Screen's sound");
    expect(soundName(r, { ...screen, role: 'camera', label: 'capture.source.camera' })).toBe('Camera sound');
  });

  it('words an undo label that names a sound in the language shown', () => {
    const r = twoTakes().recordings[0];
    expect(labelText({ id: 'undo.source.mute', params: { source: soundParam(r, r.sources[2]) } })).toBe(
      'Mute Computer sound',
    );
  });
});
