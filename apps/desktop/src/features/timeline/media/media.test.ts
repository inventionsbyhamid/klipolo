import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ThumbStrip } from '../../../bindings/ThumbStrip';
import { pxToUs, usToPx } from '../scale';
import { CACHED_SPRITES, filmTiles, MAX_SPRITES } from './drawFilmstrip';
import { drawWaveform, peakLevel } from './drawWaveform';
import { anchorTransform } from './anchor';
import { createLru } from './peaksCache';
import { DECODE_TIMEOUT_MS, spriteCache } from './spriteCache';

vi.mock('../../../bridge/commands', () => ({ mediaUrl: (path: string) => path }));

describe('waveforms', () => {
  it('reads the coarsest level with at least one bucket per device pixel', () => {
    expect(peakLevel(40, 2)).toBe(0); // 80 px/s needs level 0 (187.5 buckets/s)
    expect(peakLevel(8, 2)).toBe(1); // 16 px/s: level 1 (46.9/s)
    expect(peakLevel(0.3, 2)).toBe(3); // the hour fitted: level 3 (2.9/s), grouped per pixel
  });

  it('draws one column per device pixel, scaled by volume and fades', () => {
    const ctx = {
      fillRect: vi.fn(),
      set fillStyle(_: string) {},
      clearRect: vi.fn(),
    } as unknown as CanvasRenderingContext2D;
    // Two buckets per pixel, mono, full scale everywhere.
    const peaks = new Int16Array(Array.from({ length: 40 }, (_, i) => (i % 2 ? 32767 : -32767)));
    drawWaveform(
      ctx,
      peaks,
      1,
      { columns: 10, bucketsPerColumn: 2, firstBucket: 0, height: 20, gainAt: (x) => (x < 5 ? 1 : 0.5) },
      { color: 'rgb(0,0,0)', baseline: 'centre' },
    );
    const calls = vi.mocked(ctx.fillRect).mock.calls;
    expect(calls).toHaveLength(10);
    expect(calls[0]).toEqual([0, 0, 1, 20]);
    expect(calls[9]).toEqual([9, 5, 1, 10]);
  });

  it('keeps at most the newest entries', () => {
    const lru = createLru<number>(2);
    lru.set('a', 1);
    lru.set('b', 2);
    lru.get('a');
    lru.set('c', 3);
    expect([lru.get('a'), lru.get('b'), lru.get('c')]).toEqual([1, undefined, 3]);
  });
});

describe('waveform loudness', () => {
  it('draws heights in decibels, so quiet speech still reads', () => {
    const ctx = { fillRect: vi.fn(), set fillStyle(_: string) {} } as unknown as CanvasRenderingContext2D;
    // Full scale, −24 dBFS (half of a 48 dB scale) and silence, one bucket per column.
    const peaks = new Int16Array([-32767, 32767, -2068, 2068, 0, 0]);
    const map = { columns: 3, bucketsPerColumn: 1, firstBucket: 0, height: 20, gainAt: () => 1 };
    drawWaveform(ctx, peaks, 1, map, { color: 'rgb(0,0,0)', baseline: 'bottom', floorDb: 48 });
    expect(vi.mocked(ctx.fillRect).mock.calls).toEqual([
      [0, 0, 1, 20],
      [1, 10, 1, 10],
      [2, 19, 1, 1],
    ]);
  });
});

describe('filmstrips', () => {
  /** A recording's strip, every chunk scanned. */
  const strip = (hours: number): ThumbStrip => ({
    tileW: 128,
    tileH: 72,
    cols: 8,
    perChunk: 64,
    slotUs: 1_000_000,
    chunks: Array.from({ length: Math.ceil((hours * 3600) / 64) }, (_, index) => ({
      index,
      path: `/thumbs/c-${index}.jpg`,
      full: false,
    })),
  });
  const tileW = (128 * 39) / 72;

  /**
   * One paint of a clip of a file `hours` long, its footage from `inS` to `outS` seconds, at the timeline's start:
   * `viewPx` of lanes, the window the view and one view each side (`useViewport`), the group span the window's at most
   * the file's (`BlockMedia`). Each tile's frame (slot) and the second under it.
   */
  function paint(hours: number, viewPx: number, pps: number, scrollPx = 0, inS = 0, outS = hours * 3600) {
    const viewUs = pxToUs(viewPx, pps);
    const fromUs = Math.max(0, pxToUs(scrollPx, pps) - viewUs);
    const toUs = pxToUs(scrollPx, pps) + 2 * viewUs;
    const durationUs = (outS - inS) * 1e6;
    const lastUs = outS * 1e6 - 1;
    const sourceAt = (x: number) => Math.min(inS * 1e6 + pxToUs(x, pps), lastUs);
    const x0 = usToPx(fromUs, pps);
    const map = {
      x0,
      widthPx: usToPx(Math.min(durationUs, toUs) - fromUs, pps),
      heightPx: 39,
      blockPx: usToPx(durationUs, pps),
      spanUs: Math.min(3 * viewUs, hours * 3600e6),
      usPerPx: 1e6 / pps,
      sourceAt,
    };
    return filmTiles(strip(hours), map).map((tile) => {
      const k = Math.round((tile.x + x0) / tileW);
      const middle = (k * tileW + Math.max(k * tileW, Math.min((k + 1) * tileW, map.blockPx))) / 2;
      const slot = tile.chunk * 64 + tile.row * 8 + tile.col;
      return { chunk: tile.chunk, slot, at: Math.floor(sourceAt(middle) / 1e6) };
    });
  }
  const sprites = (tiles: { chunk: number }[]) => new Set(tiles.map((tile) => tile.chunk));

  it('zoomed out, a long recording shows frames near their time, from its own footage and at most 8 sprites', () => {
    // An hour and two hours fitted to a 2560 px display's lanes and a MacBook's, and at 1 px/s. A tile shows the frame
    // nearest its time in its group (at most six groups over the file): within half a group, 5.5 minutes for an hour
    // and 11 for two (it was 14 and 29 with groups sized from the three-view window).
    for (const viewPx of [2432, 1256])
      for (const hours of [1, 2])
        for (const pps of [viewPx / (hours * 3600), 1]) {
          const tiles = paint(hours, viewPx, pps);
          const name = `${hours} h at ${pps.toFixed(3)} px/s on ${viewPx} px`;
          expect(sprites(tiles).size, name).toBeLessThanOrEqual(MAX_SPRITES);
          if (pps < 1) expect(sprites(tiles).size, name).toBeGreaterThanOrEqual(5);
          const error = Math.max(...tiles.map((t) => Math.abs(t.slot - t.at)));
          expect(error, name).toBeLessThanOrEqual(hours === 1 ? 5.5 * 60 : 11 * 60);
          for (const t of tiles) expect(t.slot, name).toBeLessThan(hours * 3600);
        }
  });

  it('never shows footage trimmed off the clip', () => {
    // An hour's recording trimmed to 10:00 to 50:00, fitted and at 1 px/s.
    for (const pps of [2432 / 2400, 1])
      for (const t of paint(1, 2432, pps, 0, 600, 3000)) {
        expect(t.slot).toBeGreaterThanOrEqual(600);
        expect(t.slot).toBeLessThan(3000);
      }
  });

  it('a window step reuses the sprites on screen, at every zoom', () => {
    // Two hours scrolled half a view at a time, at 61 zooms from the hour fitted to 12 px/s, on three display widths:
    // at most two sprites the last paint didn't use, so the cache never refetches what is on screen.
    for (const viewPx of [2432, 1256, 972])
      for (let i = 0; i <= 60; i++) {
        const pps = (viewPx / 7200) * (12 / (viewPx / 7200)) ** (i / 60);
        let before: Set<number> | null = null;
        for (let step = 0; (step * viewPx) / 2 < 7200 * pps; step++) {
          const now = sprites(paint(2, viewPx, pps, (step * viewPx) / 2));
          expect(now.size).toBeLessThanOrEqual(MAX_SPRITES);
          if (before) expect([...now].filter((chunk) => !before!.has(chunk)).length).toBeLessThanOrEqual(2);
          before = now;
        }
      }
  });

  it('zoomed in, every tile shows its own second', () => {
    // An hour's recording at 40 px/s on a MacBook: one tile per 69 px, each the slot under its middle.
    for (const t of paint(1, 1256, 40)) expect(t.slot).toBe(t.at);
  });

  it('exact frames from about 16.5 px/s on a 2,432 px lane, still at most 8 sprites', () => {
    // At 17 px/s the three-view window spans 6.7 chunks and a tile 0.06: exact (grouped in twos before, from 19 px/s
    // down). At 16 px/s (7.2 chunks) it groups.
    for (let step = 0; step < 12; step++) {
      const tiles = paint(1, 2432, 17, step * 1216);
      for (const t of tiles) expect(t.slot).toBe(t.at);
      expect(sprites(tiles).size).toBeLessThanOrEqual(MAX_SPRITES);
    }
    expect(paint(1, 2432, 16).some((t) => t.slot !== t.at)).toBe(true);
  });

  it('a block narrower than a tile shows its middle, not its last second', () => {
    // 3 s at 10 px/s (30 px) from 100 s: the middle, 101.5 s.
    const map = {
      x0: 0,
      widthPx: 30,
      heightPx: 39,
      blockPx: 30,
      spanUs: 360e6,
      usPerPx: 1e5,
      sourceAt: (x: number) => 100e6 + pxToUs(x, 10),
    };
    const [tile] = filmTiles(strip(1), map);
    expect(tile.chunk * 64 + tile.row * 8 + tile.col).toBe(101);
  });
});

describe('a canvas anchored to what it drew', () => {
  const drawn = { offUs: 10e6, srcUs: 10e6, pps: 40, speed: 1 };

  it('stays put where it was drawn', () => {
    expect(anchorTransform(drawn, { pps: 40, sourceInUs: 0, speed: 1 })).toBe('');
  });

  it('stretches with a zoom over the same time', () => {
    // Drawn from 10 s at 40 px/s (400 px); at 80 px/s that time is at 800 px, twice as wide.
    expect(anchorTransform(drawn, { pps: 80, sourceInUs: 0, speed: 1 })).toBe('translateX(400px) scaleX(2)');
  });

  it('stays over its source when a start trim moves where the block begins in it', () => {
    // The start trimmed by 2 s: source 10 s is 8 s into the block now, 80 px left of where it was drawn.
    expect(anchorTransform(drawn, { pps: 40, sourceInUs: 2e6, speed: 1 })).toBe(
      'translateX(-80px) scaleX(1)',
    );
  });
});

describe('sprites', () => {
  /** Decodes the test settles itself: each path's start, and its `done`. */
  let started: string[];
  let finish: Map<string, () => void>;
  const decode = HTMLImageElement.prototype.decode;
  beforeEach(() => {
    started = [];
    finish = new Map();
    HTMLImageElement.prototype.decode = function (this: HTMLImageElement) {
      const name = this.src.split('/').pop()!;
      started.push(name);
      return new Promise<void>((done) => finish.set(name, done));
    };
  });
  afterEach(() => {
    HTMLImageElement.prototype.decode = decode;
    vi.useRealTimers();
  });
  const turn = () => new Promise((done) => setTimeout(done, 0));
  /** Settles every decode started, and those that start meanwhile, until all of `loads` have. */
  async function drain(loads: Promise<unknown>[]) {
    const settled = Promise.allSettled(loads);
    let done = false;
    void settled.then(() => (done = true));
    while (!done) {
      for (const [name, end] of finish) {
        finish.delete(name);
        end();
      }
      await turn();
    }
  }

  it('decode three at a time, the newest request first, and stay decoded for the next paint', async () => {
    const loads = Array.from({ length: 6 }, (_, i) => spriteCache.load(`/sprites/${i}.jpg`));
    expect(started).toEqual(['0.jpg', '1.jpg', '2.jpg']);
    finish.get('0.jpg')!();
    await turn();
    expect(started).toEqual(['0.jpg', '1.jpg', '2.jpg', '5.jpg']);
    await drain(loads);
    expect(started).toHaveLength(6);
    expect(spriteCache.image('/sprites/0.jpg')).toBeDefined();
  });

  it('one asked for again while it waits goes next', async () => {
    const loads = Array.from({ length: 6 }, (_, i) => spriteCache.load(`/again/${i}.jpg`));
    // 3, 4 and 5 wait; a block scrolled back into view asks for 3 again.
    loads.push(spriteCache.load('/again/3.jpg'));
    finish.get('0.jpg')!();
    await turn();
    expect(started.at(-1)).toBe('3.jpg');
    await drain(loads);
  });

  it('a long queue keeps its order as sprites are asked for again: each move is O(1)', async () => {
    // Three decode and 300 wait (oldest 3, newest 302). The odd ones are asked for again, in order: they go first, the
    // last asked first, then the rest, newest first.
    const loads = Array.from({ length: 303 }, (_, i) => spriteCache.load(`/queue/${i}.jpg`));
    for (let i = 3; i < 303; i += 2) loads.push(spriteCache.load(`/queue/${i}.jpg`));
    await drain(loads);
    const odd = Array.from({ length: 150 }, (_, k) => `${301 - 2 * k}.jpg`);
    const even = Array.from({ length: 150 }, (_, k) => `${302 - 2 * k}.jpg`);
    expect(started.slice(3)).toEqual([...odd, ...even]);
  });

  it('a decode that hangs gives its turn back after 10 s, and is tried again on the next ask', async () => {
    vi.useFakeTimers();
    const hung = [0, 1, 2].map((i) =>
      spriteCache.load(`/hung/${i}.jpg`).catch((error: Error) => error.message),
    );
    const next = spriteCache.load('/hung/3.jpg');
    expect(started).toEqual(['0.jpg', '1.jpg', '2.jpg']);
    await vi.advanceTimersByTimeAsync(DECODE_TIMEOUT_MS);
    expect(await Promise.all(hung)).toEqual(Array(3).fill('sprite_decode: timeout'));
    expect(started).toContain('3.jpg');
    finish.get('3.jpg')!();
    await expect(next).resolves.toBeDefined();
    // Not cached, nor stuck in flight: asking again decodes it again.
    void spriteCache.load('/hung/0.jpg');
    expect(started.filter((name) => name === '0.jpg')).toHaveLength(2);
    finish.get('0.jpg')!();
    await vi.advanceTimersByTimeAsync(0);
  });

  it('a decode for a block that scrolled away never evicts a sprite on screen', async () => {
    // The cache full of sprites on-screen blocks draw from.
    const shown = Array.from({ length: CACHED_SPRITES }, (_, i) => `/shown/${i}.jpg`);
    await drain(shown.map((path) => spriteCache.load(path)));
    // A request whose block scrolls away while it decodes, and one whose block is gone before its turn.
    let live = true;
    const late = spriteCache.load('/late/late.jpg', () => live);
    live = false;
    await drain([late]);
    expect(started).toContain('late.jpg');
    expect(spriteCache.image('/late/late.jpg')).toBeUndefined();
    for (const path of shown) expect(spriteCache.image(path), path).toBeDefined();
    const skipped = spriteCache.load('/late/skipped.jpg', () => false);
    await expect(skipped).rejects.toThrow();
    expect(started).not.toContain('skipped.jpg');
  });
});
