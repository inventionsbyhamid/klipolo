import { describe, expect, it } from 'vitest';
import { historyAction } from './history';

describe("chat's labels for a recording's camera and sounds", () => {
  it('names each change as the history shows it', () => {
    expect(historyAction('undo', 'chat.setCameraMode', { mode: 'hidden' })).toBe('Undo hide the camera');
    expect(historyAction('undo', 'chat.setCameraMode', { mode: 'splitBottom' })).toBe(
      'Undo put the camera under the screen',
    );
    expect(historyAction('redo', 'chat.setCameraMode', { mode: 'bubble' })).toBe('Redo show the camera as a bubble');
    expect(historyAction('undo', 'chat.adjustCamera')).toBe('Undo adjust the camera');
    expect(historyAction('undo', 'chat.detachCamera')).toBe('Undo separate the camera');
    expect(historyAction('undo', 'chat.muteSource', { source: 'mic', mute: 'on' })).toBe('Undo mute the mic');
    expect(historyAction('redo', 'chat.muteSource', { source: 'computerSound', mute: 'off' })).toBe(
      'Redo unmute the computer sound',
    );
    expect(historyAction('undo', 'chat.setSourceVolume', { source: 'gameSound', percent: 50 })).toBe(
      'Undo set the game sound to 50%',
    );
    expect(historyAction('undo', 'chat.separateSound', { source: 'mic' })).toBe('Undo separate the mic');
  });

  it('names the app that made the change', () => {
    expect(historyAction('undo', 'chat.muteSource', { source: 'videoSound', mute: 'on', client: 'Example Client' })).toBe(
      'Undo mute the camera sound (from Example Client)',
    );
  });
});
