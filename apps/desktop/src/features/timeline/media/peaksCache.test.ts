import { expect, it, vi } from 'vitest';

vi.mock('../../../bridge/media', () => ({
  media: {
    peaksInfo: vi.fn(async () => ({ channels: 1, levels: 4, bucket: 256, frames: 48_000 * 10 })),
    peaks: vi.fn(async () => new Int16Array(4096 * 2).buffer),
    thumbs: vi.fn(async () => null),
  },
}));
const { media } = await import('../../../bridge/media');
const { loadWave } = await import('./peaksCache');

it("a file's sound cache built again (a new media version) is read again, not taken from the old ranges", async () => {
  const path = '/media/rebuilt.wav';
  await loadWave(path, 0, 0, 2_000_000, 0);
  await loadWave(path, 0, 0, 2_000_000, 0);
  // The same version: one read, the rest from the cache.
  expect(media.peaks).toHaveBeenCalledTimes(1);
  // The engine wrote the file's peaks again (its media event bumped the version): its ranges are read again.
  const wave = await loadWave(path, 1, 0, 2_000_000, 0);
  expect(media.peaks).toHaveBeenCalledTimes(2);
  expect(wave?.key).toContain('#1');
});
