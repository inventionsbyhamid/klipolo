import { beforeEach, describe, expect, it, vi } from 'vitest';

/*
 * The safe area the person picked, and the one picked last, are kept on this Mac: the next launch shows the same guide,
 * and a drag with it off shows the same platform.
 */

const KEY = 'klipolo.safeArea';
const launch = async () => {
  vi.resetModules();
  return (await import('./ui')).useUi;
};

beforeEach(() => localStorage.clear());

describe('the safe area kept between launches', () => {
  it('starts off, with all platforms for a drag, the first time', async () => {
    expect((await launch()).getState()).toMatchObject({ safeArea: 'off', lastSafeArea: 'all' });
  });

  it('the next launch shows the area picked, and the one picked last when it was turned off', async () => {
    let ui = await launch();
    ui.getState().setSafeArea('tiktok');
    expect((await launch()).getState()).toMatchObject({ safeArea: 'tiktok', lastSafeArea: 'tiktok' });
    ui = await launch();
    ui.getState().setSafeArea('off');
    expect((await launch()).getState()).toMatchObject({ safeArea: 'off', lastSafeArea: 'tiktok' });
  });

  it('a kept value it doesn’t know, or storage it can’t read, starts as the first time', async () => {
    localStorage.setItem(KEY, JSON.stringify({ safeArea: 'snapchat', lastSafeArea: 'reels' }));
    expect((await launch()).getState()).toMatchObject({ safeArea: 'off', lastSafeArea: 'reels' });
    localStorage.setItem(KEY, '{not json');
    expect((await launch()).getState()).toMatchObject({ safeArea: 'off', lastSafeArea: 'all' });
  });
});
