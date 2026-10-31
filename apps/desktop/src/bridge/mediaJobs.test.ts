import { describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
const { listen } = await import('@tauri-apps/api/event');
const { mediaJobs } = await import('./mediaJobs');

describe('the editing copies bridge', () => {
  it('stops following the list quietly where Tauri’s events can’t be removed', async () => {
    // As Tauri's own does when its event plugin isn't there (the page's tests): the removal rejects.
    const off = vi.fn(() => Promise.reject(new TypeError('unregisterListener is not a function')));
    vi.mocked(listen).mockResolvedValue(off as never);
    const stop = await mediaJobs.onChange(() => undefined);
    expect(() => stop()).not.toThrow();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(off).toHaveBeenCalledOnce();
  });
});
