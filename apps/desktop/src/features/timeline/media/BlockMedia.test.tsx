import { act, waitFor } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { beforeAll, expect, it, onTestFinished, vi } from 'vitest';
import { media } from '../../../bridge/media';
import { useMedia } from '../../../state/media';
import { renderWithProviders } from '../../../test/render';
import { clearShifts, registerBlock, sizeBlock } from '../drag/registry';
import { CACHED_SPRITES } from './drawFilmstrip';
import { BlockMedia } from './BlockMedia';
import type { BlockRuns } from './runs';
import { themeChanged } from './resolveToken';
import { createZoomRest, type ZoomRest } from './zoomRest';
import { ViewportContext, ZoomRestContext } from './ViewportContext';

vi.mock('../../../bridge/commands', () => ({ mediaUrl: (path: string) => path }));
vi.mock('../../../bridge/media', () => ({
  media: {
    thumbs: vi.fn().mockResolvedValue({
      tileW: 128,
      tileH: 72,
      cols: 8,
      perChunk: 64,
      slotUs: 1_000_000,
      chunks: [{ index: 0, path: '/c/0.jpg', full: false }],
    }),
    peaksInfo: vi.fn().mockResolvedValue({ channels: 1, levels: 4, bucket: 256, frames: 2_880_000 }),
    peaks: vi.fn().mockResolvedValue(new Int16Array(8192).buffer),
  },
}));

// jsdom has no 2D canvas: a context that records what is drawn.
const ctx = {
  save: vi.fn(),
  restore: vi.fn(),
  beginPath: vi.fn(),
  rect: vi.fn(),
  clip: vi.fn(),
  setTransform: vi.fn(),
  clearRect: vi.fn(),
  fillRect: vi.fn(),
  drawImage: vi.fn(),
  globalAlpha: 1,
  fillStyle: '',
};
beforeAll(() => {
  HTMLCanvasElement.prototype.getContext = (() => ctx) as unknown as HTMLCanvasElement['getContext'];
});

it('asks for the thumbnails and waveform of what it shows, and again when the file changes', async () => {
  renderWithProviders(
    <ViewportContext value={{ fromUs: 0, toUs: 60_000_000, widthPx: 1200, scrollPx: 0 }}>
      <BlockMedia
        kind="clip"
        thumbPath="/media/screen"
        peakPaths={['/media/mic']}
        startUs={0}
        durationUs={20_000_000}
        sourceInUs={0}
        speed={1}
        volume={1}
        fadeInUs={0}
        fadeOutUs={0}
        muted={false}
        pps={40}
      />
    </ViewportContext>,
  );
  await waitFor(() => expect(media.thumbs).toHaveBeenCalledWith('/media/screen'));
  // jsdom's devicePixelRatio is 1: 40 px/s reads level 1 (46.9 buckets/s).
  expect(media.peaks).toHaveBeenCalledWith('/media/mic', 1, 0, 4096);
  act(() => useMedia.getState().bump('/media/screen'));
  await waitFor(() => expect(media.thumbs).toHaveBeenCalledTimes(2));
});

it('keeps the plain colour when the media cannot load', async () => {
  // As in a test without this mock, or the dev fake: every load fails. Vitest fails the run on an unhandled rejection.
  // Paths of their own, so nothing the first test cached can stand in.
  vi.mocked(media.thumbs).mockRejectedValueOnce(new Error('Fake bridge: unhandled command media_thumbs'));
  vi.mocked(media.peaksInfo).mockRejectedValueOnce(
    new Error('Fake bridge: unhandled command media_peaks_info'),
  );
  renderWithProviders(
    <ViewportContext value={{ fromUs: 0, toUs: 60_000_000, widthPx: 1200, scrollPx: 0 }}>
      <BlockMedia
        kind="clip"
        thumbPath="/media/broken"
        peakPaths={['/media/broken-mic']}
        startUs={0}
        durationUs={20_000_000}
        sourceInUs={0}
        speed={1}
        volume={1}
        fadeInUs={0}
        fadeOutUs={0}
        muted={false}
        pps={40}
      />
    </ViewportContext>,
  );
  await waitFor(() => expect(media.peaksInfo).toHaveBeenCalledWith('/media/broken-mic'));
  expect(media.peaks).not.toHaveBeenCalledWith(
    '/media/broken-mic',
    expect.anything(),
    expect.anything(),
    expect.anything(),
  );
});

it('draws the waveform along the bottom of the clip, one column per device pixel', async () => {
  renderWithProviders(
    <ViewportContext value={{ fromUs: 0, toUs: 60_000_000, widthPx: 1200, scrollPx: 0 }}>
      <BlockMedia
        kind="clip"
        thumbPath="/media/silent"
        peakPaths={['/media/silent-mic']}
        startUs={0}
        durationUs={20_000_000}
        sourceInUs={0}
        speed={1}
        volume={1}
        fadeInUs={0}
        fadeOutUs={0}
        muted={false}
        pps={40}
      />
    </ViewportContext>,
  );
  // 20 s at 40 px/s: 800 columns of silence, 1 px each, at the bottom of the clip's 66 px: its 25 px strip (38 %) under
  // the 41 px filmstrip (62 %).
  await waitFor(() => expect(ctx.fillRect).toHaveBeenCalledWith(799, 24, 1, 1));
  expect(ctx.setTransform).toHaveBeenCalledWith(1, 0, 0, 1, 0, 41);
  expect(ctx.fillRect.mock.calls.length % 800).toBe(0);
});

/** A 10 s block at 40 px/s in a 0 to 60 s window, as `props` and `window` change it. */
function block(
  props: Partial<ComponentProps<typeof BlockMedia>>,
  window = { fromUs: 0, toUs: 60_000_000 },
  rest: ZoomRest | null = null,
) {
  return (
    <ViewportContext value={{ ...window, widthPx: 1200, scrollPx: 0 }}>
      <ZoomRestContext value={rest}>
        <BlockMedia
          kind="clip"
          peakPaths={[]}
          startUs={0}
          durationUs={10_000_000}
          sourceInUs={0}
          speed={1}
          volume={1}
          fadeInUs={0}
          fadeOutUs={0}
          muted={false}
          pps={40}
          {...props}
        />
      </ZoomRestContext>
    </ViewportContext>
  );
}
const sleep = (ms: number) => act(() => new Promise((done) => setTimeout(done, ms)));

it('stretches its picture with a pinch, and redraws once at the zoom it rests on', async () => {
  const rest = createZoomRest();
  const sound = (pps: number) =>
    block({ kind: 'audio', peakPaths: ['/media/music'], height: 32, pps }, undefined, rest);
  const view = renderWithProviders(sound(40));
  await waitFor(() => expect(ctx.fillRect).toHaveBeenCalledWith(399, 16, 1, 1));
  const canvas = document.querySelector('canvas')!;
  expect([canvas.style.left, canvas.style.width, canvas.style.transform]).toEqual(['-3px', '400px', '']);
  ctx.fillRect.mockClear();
  // Two steps of a pinch, a frame apart: the 40 px/s picture stretches over the same time, with the ruler.
  view.rerender(sound(50));
  view.rerender(sound(60));
  expect(canvas.style.transform).toBe('translateX(0px) scaleX(1.5)');
  await sleep(40);
  expect(ctx.fillRect).not.toHaveBeenCalled();
  // The zoom rests: 10 s at 60 px/s draws once, in its own place.
  await waitFor(() => expect(ctx.fillRect).toHaveBeenCalledTimes(600));
  expect([canvas.style.width, canvas.style.transform]).toEqual(['600px', '']);
  await sleep(150);
  expect(ctx.fillRect).toHaveBeenCalledTimes(600);
});

it('stays over the time it drew when the window steps, until its new part is drawn', async () => {
  // A 100 s clip at 40 px/s, as the window steps from 0 to 60 s to 30 to 90 s.
  const long = (fromUs: number) =>
    block({ peakPaths: ['/media/long-mic'], durationUs: 100_000_000 }, { fromUs, toUs: fromUs + 60_000_000 });
  const view = renderWithProviders(long(0));
  const canvas = document.querySelector('canvas')!;
  await waitFor(() => expect(canvas.style.width).toBe('2400px'));
  expect(canvas.style.left).toBe('-3px');
  view.rerender(long(30_000_000));
  expect([canvas.style.left, canvas.style.width]).toEqual(['-3px', '2400px']);
  await waitFor(() => expect(canvas.style.left).toBe('1197px'));
});

it('draws the voice over a lighter system sound', async () => {
  const alphas: number[] = [];
  ctx.fillRect.mockImplementation(() => alphas.push(ctx.globalAlpha));
  renderWithProviders(block({ peakPaths: ['/media/rec-mic', '/media/rec-system'] }));
  // 10 s at 40 px/s: the system sound's 400 columns first, then the mic's over them.
  await waitFor(() => expect(alphas.length).toBeGreaterThanOrEqual(800));
  expect(alphas.slice(0, 800)).toEqual([...Array(400).fill(0.3), ...Array(400).fill(0.7)]);
  ctx.fillRect.mockReset();
});

it('stays put in time while a start edge is dragged', () => {
  // The block's left edge moves 40 px right: its media moves 40 px back (no redraw).
  const el = document.createElement('div');
  el.dataset.left = '100';
  const forget = registerBlock('trimmed', el);
  sizeBlock('trimmed', 140, 60);
  expect(el.style.getPropertyValue('--trim-dx')).toBe('40px');
  clearShifts();
  expect(el.style.getPropertyValue('--trim-dx')).toBe('');
  forget();
  renderWithProviders(block({ peakPaths: ['/media/trimmed'] }));
  expect(document.querySelector('canvas')!.style.translate).toBe('calc(0px - var(--trim-dx, 0px)) 0');
});

/** Every file asked for has `chunks` scanned 64 s chunks; sprites decode at once. Both go back after the test. */
function longFiles(chunks: number) {
  const strip = vi.mocked(media.thumbs).getMockImplementation();
  const decodeBefore = HTMLImageElement.prototype.decode;
  onTestFinished(() => {
    HTMLImageElement.prototype.decode = decodeBefore;
    if (strip) vi.mocked(media.thumbs).mockImplementation(strip);
  });
  vi.mocked(media.thumbs).mockImplementation(async (path) => ({
    tileW: 128,
    tileH: 72,
    cols: 8,
    perChunk: 64,
    slotUs: 1_000_000,
    chunks: Array.from({ length: chunks }, (_, index) => ({
      index,
      path: `${path}/c-${index}.jpg`,
      full: false,
    })),
  }));
  const decode = vi.fn(() => Promise.resolve());
  HTMLImageElement.prototype.decode = decode;
  return { decode };
}
const drewFrom = (file: string) =>
  ctx.drawImage.mock.calls.some(([img]) => (img as HTMLImageElement).src.includes(file));
const still = { sourceInUs: 0, speed: 1, volume: 1, fadeInUs: 0, fadeOutUs: 0, muted: false };

it('a fitted hour shows frames from across the file, not two sprites', async () => {
  // An hour fitted to a MacBook's lanes (1,256 px): chunk groups sized by the file, not the three-view window.
  const { decode } = longFiles(57);
  const [hour, pps] = [3_600_000_000, 1256 / 3600];
  renderWithProviders(
    <ViewportContext value={{ fromUs: 0, toUs: 2 * hour, widthPx: 1384, scrollPx: 0 }}>
      <BlockMedia
        kind="clip"
        thumbPath="/media/fitted"
        filmUs={hour}
        peakPaths={[]}
        startUs={0}
        durationUs={hour}
        pps={pps}
        {...still}
      />
    </ViewportContext>,
  );
  await waitFor(() => expect(drewFrom('fitted')).toBe(true));
  expect(decode).toHaveBeenCalledTimes(6);
});

it('two long recordings on screen repaint from the sprite cache', async () => {
  // A screen clip and a camera layer, both two hours long and scanned, every track showing on a MacBook (1,256 px of
  // lanes) at 0.65 px/s, scrolled to 48 minutes: each draws from 7 sprites, 14 in all (more than the cache's old 12).
  const { decode } = longFiles(113);
  const [hours, pps, viewUs] = [2 * 3_600_000_000, 0.65, (1256 / 0.65) * 1e6];
  const visibleUs = (1884 / pps) * 1e6;
  const window = { fromUs: visibleUs - viewUs, toUs: visibleUs + 2 * viewUs, widthPx: 1384, scrollPx: 1884 };
  const film = { peakPaths: [], startUs: 0, durationUs: hours, filmUs: hours, pps, ...still };
  renderWithProviders(
    <ViewportContext value={window}>
      <BlockMedia kind="clip" thumbPath="/media/long-screen" {...film} />
      <BlockMedia kind="video" thumbPath="/media/long-camera" height={28} {...film} />
    </ViewportContext>,
  );
  await waitFor(() => expect(drewFrom('long-screen') && drewFrom('long-camera')).toBe(true));
  // At most 8 sprites a file; together they fit the cache.
  const decoded = decode.mock.calls.length;
  expect(decoded).toBe(14);
  expect(decoded).toBeLessThanOrEqual(CACHED_SPRITES);
  // A theme change repaints both from the cache: nothing is decoded again.
  ctx.drawImage.mockClear();
  act(() => themeChanged());
  await waitFor(() => expect(drewFrom('long-screen') && drewFrom('long-camera')).toBe(true));
  expect(decode).toHaveBeenCalledTimes(decoded);
});

it("draws each run of a recording from its own file, at its place and at its device's rate", async () => {
  const S = 1_000_000;
  vi.mocked(media.peaksInfo).mockResolvedValueOnce({
    channels: 1,
    levels: 4,
    bucket: 256,
    frames: 48_000 * 7_300,
  });
  // An hour in: the screen's encoder was rebuilt at 3,590 s (its second file from 3,592 s), and the mic's clock ran
  // 100 ppm fast for the whole take.
  const runs: BlockRuns = {
    film: [
      { path: '/media/heal-1', fromUs: 0, toUs: 3_590 * S, rate: 1, fileUs: 3_590 * S },
      { path: '/media/heal-2', fromUs: 3_592 * S, toUs: 7_200 * S, rate: 1, fileUs: 3_608 * S },
    ],
    sounds: [[{ path: '/media/heal-mic', fromUs: 0, toUs: 7_200 * S, rate: 1.0001, fileUs: 7_201 * S }]],
  };
  renderWithProviders(block({ runs, durationUs: 20 * S, sourceInUs: 3_582.5 * S }));
  await waitFor(() => expect(media.thumbs).toHaveBeenCalledWith('/media/heal-2'));
  expect(media.thumbs).toHaveBeenCalledWith('/media/heal-1');
  // Each file keeps to its part: the first up to halfway through the gap (340 px, 3,591 s), the second from there.
  await waitFor(() => expect(ctx.rect).toHaveBeenCalledWith(0, 0, 460, 41));
  expect(ctx.rect).toHaveBeenCalledWith(0, 0, 340, 41);
  expect(ctx.setTransform).toHaveBeenCalledWith(1, 0, 0, 1, 340, 0);
  // The mic reads where its fast clock is: 3,582.858 s into its file (bucket 167,946, in range 41), not 3,582.5 s
  // (bucket 167,929, range 40).
  expect(media.peaks).toHaveBeenCalledWith('/media/heal-mic', 1, 41 * 4096, 4096);
  expect(media.peaks).not.toHaveBeenCalledWith('/media/heal-mic', 1, 40 * 4096, 4096);
});
