import { describe, expect, it } from 'vitest';
import { BUILTINS } from './contrib';
import { contribName, NAMED_CONTRIBS } from './contribNames';
import { historyAction } from './history';
import { wordLabel, wordParams } from './labels';

describe('undo labels as message ids', () => {
  it('words an id and its params in the language shown', () => {
    expect(historyAction('undo', 'text.add')).toBe('Undo add text');
    expect(historyAction('redo', 'format.add.undo', { ratio: '9:16' })).toBe('Redo add 9:16 version');
    expect(historyAction('undo', 'gizmo.move.undo', { layer: 'camera' })).toBe('Undo move camera');
    expect(
      historyAction('undo', 'text.look.apply.undo', { style: { code: 'text.look.box', params: {} } }),
    ).toBe('Undo apply Box style');
    expect(historyAction('undo', null)).toBe('Undo');
    expect(wordParams({ atUs: 65_000_000, count: 3 })).toEqual({ atUs: '1:05', count: '3' });
  });

  it('names a spoken language by its code, in the language shown', () => {
    expect(wordParams({ language: 'de', file: 'intro.mov' })).toEqual({
      language: 'German',
      file: 'intro.mov',
    });
    expect(wordParams({ language: 'not a code' })).toEqual({ language: 'not a code' });
  });

  it('names built-in contributions from their id, never their English name', () => {
    expect(contribName('klipolo.builtin/effect.blur')).toBe('Blur');
    expect(wordLabel('effect.add.undo', { effect: 'klipolo.builtin/effect.vignette' })).toBe('Add Vignette');
    expect(contribName('pack://x@1/fx/glow')).toBeNull();
    expect(contribName('constructor')).toBeNull();
    // One this build lacks reads as missing, never as its id.
    expect(wordLabel('effect.remove.undo', { effect: 'pack://x@1/fx/glow' })).toBe('Remove missing effect');
    expect([...NAMED_CONTRIBS].sort()).toEqual(BUILTINS.map((def) => def.id).sort());
  });
});
