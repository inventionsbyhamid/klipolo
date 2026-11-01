import { describe, expect, it } from 'vitest';
import { historyAction } from './history';

describe("chat's labels for the tools that compose an edit", () => {
  it('names each change as the history shows it', () => {
    expect(historyAction('undo', 'chat.setMotion', { layer: 'words' })).toBe('Undo move the words');
    expect(historyAction('undo', 'chat.setMotion', { layer: 'picture' })).toBe('Undo move the picture');
    expect(historyAction('undo', 'chat.animateText', { by: 'letter' })).toBe('Undo bring the words in letter by letter');
    expect(historyAction('redo', 'chat.animateText', { by: 'word' })).toBe('Redo bring the words in word by word');
    expect(historyAction('undo', 'chat.animateText', { by: 'line' })).toBe('Undo bring the words in line by line');
    expect(historyAction('undo', 'chat.animateText', { by: 'none' })).toBe('Undo make the words plain again');
    expect(historyAction('undo', 'chat.setLayer', { layer: 'video' })).toBe('Undo change the video layer');
    expect(historyAction('undo', 'chat.setLayer', { layer: 'clip' })).toBe('Undo change the clip');
    expect(historyAction('undo', 'chat.setLayer', { layer: 'music' })).toBe('Undo change the music');
    expect(historyAction('undo', 'chat.styleText', {})).toBe('Undo restyle the words');
    expect(historyAction('undo', 'chat.removeLayer', { layer: 'picture' })).toBe('Undo remove the picture');
    expect(historyAction('undo', 'chat.removeLayer', { layer: 'music' })).toBe('Undo remove the music');
    expect(historyAction('undo', 'chat.setEffects', { target: 'clip' })).toBe('Undo change the clip’s effects');
    expect(historyAction('undo', 'chat.setEffects', { target: 'title' })).toBe('Undo change the title’s effects');
  });
});
