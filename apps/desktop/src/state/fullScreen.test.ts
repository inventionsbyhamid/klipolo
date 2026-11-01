import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fixtureSnapshot } from '../test/fixtures';

vi.mock('../bridge/appWindow', () => ({
  appWindow: {
    fullScreen: vi.fn(() => Promise.resolve(false)),
    isFullScreen: vi.fn(() => Promise.resolve(true)),
  },
}));
vi.mock('../bridge/engine', () => ({ engine: { seek: vi.fn(() => Promise.resolve()) } }));

const { appWindow } = await import('../bridge/appWindow');
const { useProject } = await import('./project');
const { FULL_SCREEN_SETTLE_MS, followFullScreen, useFullScreen } = await import('./fullScreen');

let stop = () => undefined as void;
beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  vi.mocked(appWindow.fullScreen).mockReset().mockResolvedValue(false);
  vi.mocked(appWindow.isFullScreen).mockReset().mockResolvedValue(true);
  useProject.setState({ snapshot: fixtureSnapshot(), pending: 0 });
  useFullScreen.setState({ on: false });
  stop = followFullScreen();
});
afterEach(async () => {
  stop();
  await useFullScreen.getState().leave();
  vi.useRealTimers();
});

describe('full screen preview', () => {
  it('puts the window in full screen, and takes it out again on leaving', async () => {
    await useFullScreen.getState().enter();
    expect(useFullScreen.getState().on).toBe(true);
    expect(appWindow.fullScreen).toHaveBeenLastCalledWith(true);
    await useFullScreen.getState().leave();
    expect(useFullScreen.getState().on).toBe(false);
    expect(appWindow.fullScreen).toHaveBeenLastCalledWith(false);
  });

  it('a window that was in full screen already stays there on leaving', async () => {
    vi.mocked(appWindow.fullScreen).mockResolvedValue(true);
    await useFullScreen.getState().enter();
    await useFullScreen.getState().leave();
    expect(appWindow.fullScreen).toHaveBeenCalledTimes(1);
  });

  it('toggles, as F does', async () => {
    await useFullScreen.getState().toggle();
    expect(useFullScreen.getState().on).toBe(true);
    await useFullScreen.getState().toggle();
    expect(useFullScreen.getState().on).toBe(false);
  });

  it('the preview still fills the window when the window refuses full screen', async () => {
    vi.mocked(appWindow.fullScreen).mockRejectedValue(new Error('no'));
    await useFullScreen.getState().enter();
    expect(useFullScreen.getState().on).toBe(true);
    await useFullScreen.getState().leave();
    expect(appWindow.fullScreen).toHaveBeenCalledTimes(1);
  });

  it('leaves when the window leaves full screen another way (the green button, ⌃⌘F)', async () => {
    await useFullScreen.getState().enter();
    // The window resizes as it goes in: still in full screen once it settles.
    window.dispatchEvent(new Event('resize'));
    await vi.advanceTimersByTimeAsync(FULL_SCREEN_SETTLE_MS);
    expect(useFullScreen.getState().on).toBe(true);
    // Then it comes out by itself.
    vi.mocked(appWindow.isFullScreen).mockResolvedValue(false);
    window.dispatchEvent(new Event('resize'));
    window.dispatchEvent(new Event('resize'));
    await vi.advanceTimersByTimeAsync(FULL_SCREEN_SETTLE_MS);
    expect(useFullScreen.getState().on).toBe(false);
    // It isn't asked to leave again.
    expect(appWindow.fullScreen).toHaveBeenCalledTimes(1);
    expect(appWindow.isFullScreen).toHaveBeenCalledTimes(2);
  });

  it('leaves when the project closes', async () => {
    await useFullScreen.getState().enter();
    useProject.setState({ snapshot: null });
    await vi.waitFor(() => expect(useFullScreen.getState().on).toBe(false));
    expect(appWindow.fullScreen).toHaveBeenLastCalledWith(false);
  });
});
