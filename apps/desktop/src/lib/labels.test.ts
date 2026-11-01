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
    // A video layer is named apart from the Video lane.
    expect(historyAction('undo', 'gizmo.move.undo', { layer: 'video' })).toBe('Undo move video layer');
    expect(historyAction('undo', 'gizmo.resize.undo', { layer: 'video' })).toBe('Undo resize video layer');
    expect(historyAction('undo', 'gizmo.rotate.undo', { layer: 'video' })).toBe('Undo rotate video layer');
    expect(historyAction('undo', 'gizmo.nudge.undo', { layer: 'video' })).toBe('Undo nudge video layer');
    expect(historyAction('undo', 'volume.mute.undo', { noun: 'video' })).toBe('Undo mute video layer');
    expect(historyAction('redo', 'volume.unmute.undo', { noun: 'video' })).toBe('Redo unmute video layer');
    expect(historyAction('undo', 'media.mirror.undo')).toBe('Undo mirror video layer');
    expect(historyAction('undo', 'media.unmirror.undo')).toBe('Undo unmirror video layer');
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

  it("words every change chat's compiler and picture jobs label, never as a bare Undo", () => {
    for (const tool of [
      'cutRange',
      'keepOnly',
      'removeSilences',
      'removeFillers',
      'trimClip',
      'splitAt',
      'setClip',
      'setClip.note',
      'setTransition',
      'setFormat',
      'setCaptions',
      'fixCaptionWords',
      'addMusic',
      'addSound',
      'addText',
      'addImage',
      'makeTitleCard',
      'makeSticker',
      'titleBackground',
    ]) {
      expect(historyAction('undo', `chat.${tool}`), tool).not.toBe('Undo');
    }
    expect(historyAction('undo', 'chat.addText', { text: 'Day 1 in Lisbon', preset: 'title' })).toBe(
      'Undo add the words Day 1 in Lisbon',
    );
  });

  it("words chat's clip and transition changes as the cards' own entries, with a speed's note", () => {
    const clip = (change: string) => historyAction('undo', 'chat.setClip', { clip: 'c2', change });
    expect(['speed', 'volume', 'mute', 'unmute', 'fit', 'fill', 'several'].map(clip)).toEqual([
      'Undo change speed',
      'Undo change volume',
      'Undo mute clip',
      'Undo unmute clip',
      'Undo fit video',
      'Undo fill video',
      'Undo change clip',
    ]);
    const note = (removed: number, shortened: number) =>
      historyAction('undo', 'chat.setClip.note', {
        clip: 'c2',
        change: 'speed',
        note: { code: 'timeline.refit', params: { removed, shortened } },
      });
    expect(note(1, 0)).toBe('Undo change speed. A transition is removed.');
    expect(note(0, 2)).toBe('Undo change speed. 2 transitions are shortened.');
    expect(note(2, 1)).toBe('Undo change speed. 2 transitions are removed and one is shortened.');
    const cut = (change: string, count: number) =>
      historyAction('undo', 'chat.setTransition', { after: 'all', change, count });
    expect([cut('add', 1), cut('add', 3), cut('change', 1), cut('length', 1), cut('remove', 2)]).toEqual([
      'Undo add transition',
      'Undo add 3 transitions',
      'Undo change transition',
      'Undo change transition length',
      'Undo remove 2 transitions',
    ]);
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

  it('undo_label_names_the_client', () => {
    expect(historyAction('undo', 'chat.setFormat', { format: 'portrait', client: 'Example Client' })).toBe(
      'Undo make it vertical (from Example Client)',
    );
    expect(historyAction('redo', 'chat.addText', { text: 'Day 1', preset: 'title', client: 'Muse Spark agent' })).toBe(
      'Redo add the words Day 1 (from Muse Spark agent)',
    );
    // A step folded from an app's earlier ones names the app once.
    expect(wordLabel('mcp.fromEarlier', { client: 'Example Client' })).toBe('Earlier changes from Example Client');
    expect(historyAction('undo', 'mcp.fromEarlier', { client: 'Example Client' })).toBe('Undo earlier changes from Example Client');
    // The person's own changes read as before.
    expect(historyAction('undo', 'chat.setFormat', { format: 'portrait' })).toBe('Undo make it vertical');
  });
});
