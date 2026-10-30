import { describe, expect, it, vi } from 'vitest';

const bridge = vi.hoisted(() => ({
  engineSeek: vi.fn(() => Promise.resolve()),
  engineAttach: vi.fn(),
  projectSummary: vi.fn(),
  enginePlay: vi.fn(),
  enginePause: vi.fn(),
  engineSetVisible: vi.fn(() => Promise.resolve()),
}));
vi.mock('../bridge/engine', () => bridge);

import { getSession, pause, play } from './session';

describe('session transport', () => {
  it('says why when pausing fails, without an unhandled rejection', async () => {
    bridge.enginePause.mockRejectedValueOnce("The preview isn't ready yet.");
    await expect(pause()).resolves.toBeUndefined();
    expect(getSession().message).toBe("The preview isn't ready yet.");
  });

  it('says why when playing fails', async () => {
    bridge.enginePlay.mockRejectedValueOnce('Add a video to play it.');
    await expect(play()).resolves.toBeUndefined();
    expect(getSession().message).toBe('Add a video to play it.');
    expect(getSession().playing).toBe(false);
  });
});
