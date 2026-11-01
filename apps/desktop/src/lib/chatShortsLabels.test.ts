import { describe, expect, it } from 'vitest';
import { historyAction } from './history';

describe("chat's labels for a layer's animation and the auto zooms", () => {
  it('names each change as the history shows it', () => {
    expect(historyAction('undo', 'chat.animate', { layer: 'words' })).toBe('Undo animate the words');
    expect(historyAction('undo', 'chat.animate', { layer: 'picture', sound: 'Pop' })).toBe('Undo animate the picture');
    expect(historyAction('redo', 'chat.animate', { layer: 'video' })).toBe('Redo animate the video layer');
    expect(historyAction('undo', 'chat.animate', { layer: 'title' })).toBe('Undo animate the title');
    expect(historyAction('undo', 'chat.autoZoom', { on: 'on', count: 2 })).toBe('Undo zoom in on the clicks');
    expect(historyAction('undo', 'chat.autoZoom', { on: 'off', count: 2 })).toBe('Undo remove the auto zooms');
  });
});
