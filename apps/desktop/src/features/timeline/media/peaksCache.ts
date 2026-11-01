import type { PeaksInfo } from '../../../bindings/PeaksInfo';
import { media } from '../../../bridge/media';
import { bucketRate } from './drawWaveform';

/** Buckets in one peaks range: what one `media_peaks` call reads and one cache entry holds (32 KB of stereo). */
export const CHUNK_BUCKETS = 4096;

export interface Lru<V> {
  /** The value, which becomes the newest entry. */
  get(key: string): V | undefined;
  /** Adds or refreshes `key` as the newest entry, dropping the oldest past the limit. */
  set(key: string, value: V): void;
  delete(key: string): void;
  /** How many entries it holds. */
  size(): number;
}

/** A `Map` kept in use order (oldest first) and at most `limit` entries long. */
export function createLru<V>(limit: number): Lru<V> {
  const map = new Map<string, V>();
  return {
    get(key) {
      const value = map.get(key);
      if (value !== undefined) {
        map.delete(key);
        map.set(key, value);
      }
      return value;
    },
    set(key, value) {
      map.delete(key);
      map.set(key, value);
      for (const oldest of map.keys()) {
        if (map.size <= limit) break;
        map.delete(oldest);
      }
    },
    delete(key) {
      map.delete(key);
    },
    size: () => map.size,
  };
}

/**
 * What a request hands out until it settles, so many blocks asking for the same thing at once make one call. A
 * failed request is dropped, so the next ask (a redraw, a media event) tries again.
 */
export function once<V>(inFlight: Map<string, Promise<V>>, key: string, load: () => Promise<V>): Promise<V> {
  let pending = inFlight.get(key);
  if (!pending) {
    pending = load().finally(() => inFlight.delete(key));
    inFlight.set(key, pending);
  }
  return pending;
}

/** Decoded peak ranges in the webview: at most 64 (the webview process stays under 350 MB), about 2 MB of stereo. */
const ranges = createLru<Int16Array>(64);
const loading = new Map<string, Promise<Int16Array>>();
/** A range of `path`'s peaks as of media version `version` of it: a rebuilt sound cache is read again, as infos are. */
const rangeKey = (path: string, version: number, level: number, chunk: number) =>
  `${path}|${version}|${level}|${chunk}`;

export const peaksCache = {
  get: (path: string, version: number, level: number, chunk: number) =>
    ranges.get(rangeKey(path, version, level, chunk)),
  /** Range `chunk` of `level` (`CHUNK_BUCKETS` buckets of i16 [min, max] per channel). Rejects where there are none. */
  load(path: string, version: number, level: number, chunk: number): Promise<Int16Array> {
    const key = rangeKey(path, version, level, chunk);
    const hit = ranges.get(key);
    if (hit) return Promise.resolve(hit);
    return once(loading, key, async () => {
      const bytes = await media.peaks(path, level, chunk * CHUNK_BUCKETS, CHUNK_BUCKETS);
      // No media cache (the dev fake): nothing to draw.
      if (!bytes) throw new Error('media_peaks: null');
      const peaks = new Int16Array(bytes);
      ranges.set(key, peaks);
      return peaks;
    });
  },
};

/** A file's peaks header as of media version `version` of it (null until its sound cache is built). */
const infos = createLru<Promise<PeaksInfo | null>>(64);

export function peaksInfoOf(path: string, version: number): Promise<PeaksInfo | null> {
  const key = `${path}|${version}`;
  let info = infos.get(key);
  if (!info) {
    info = media.peaksInfo(path);
    infos.set(key, info);
    info.catch(() => infos.delete(key));
  }
  return info;
}

/**
 * `peaksInfoOf`, except that a file without peaks isn't remembered as such: the next ask reads it again. Line up by
 * hand asks this way while a file's sound cache is rebuilt; the timeline waits for the media event.
 * `reread`: what is remembered is read again too, as once the panel's own build has finished, when another reader
 * (the timeline) may have remembered "no peaks yet" from before it did.
 */
export async function peaksInfoFresh(
  path: string,
  version: number,
  reread = false,
): Promise<PeaksInfo | null> {
  if (reread) infos.delete(`${path}|${version}`);
  const info = await peaksInfoOf(path, version);
  if (!info) infos.delete(`${path}|${version}`);
  return info;
}

/** What one file's waveform draws: buckets `from`… of one level, contiguous, `channels` × [min, max] each. */
export interface Wave {
  peaks: Int16Array;
  channels: number;
  /** The first bucket's index in the file, and buckets per second of source. */
  from: number;
  rate: number;
  /** Names what was loaded (the same key is the same picture: peaks never change). */
  key: string;
}

/** Buckets [from, to) of one file's level, out of its loaded ranges (no copy when they fall in one range). */
function gather(
  ranges: Int16Array[],
  firstChunk: number,
  from: number,
  to: number,
  stride: number,
): Int16Array {
  const base = firstChunk * CHUNK_BUCKETS;
  if (ranges.length === 1) return ranges[0].subarray((from - base) * stride, (to - base) * stride);
  const out = new Int16Array((to - from) * stride);
  ranges.forEach((range, i) => {
    const start = base + i * CHUNK_BUCKETS;
    const a = Math.max(from, start);
    const b = Math.min(to, start + range.length / stride);
    if (b > a) out.set(range.subarray((a - start) * stride, (b - start) * stride), (a - from) * stride);
  });
  return out;
}

/** One file's buckets for source µs [fromUs, toUs) at `level` (its coarsest, if it has fewer); null if none. */
async function filePeaks(
  path: string,
  version: number,
  fromUs: number,
  toUs: number,
  wanted: number,
): Promise<Wave | null> {
  const info = await peaksInfoOf(path, version);
  if (!info || info.channels < 1) return null;
  const level = Math.min(wanted, info.levels - 1);
  const rate = bucketRate(info.bucket, level);
  const total = Math.ceil(info.frames / (info.bucket * 4 ** level));
  const from = Math.min(total, Math.max(0, Math.floor((fromUs / 1e6) * rate)));
  const to = Math.min(total, Math.ceil((toUs / 1e6) * rate));
  if (to <= from) return null;
  const first = Math.floor(from / CHUNK_BUCKETS);
  const loads: Promise<Int16Array>[] = [];
  for (let chunk = first; chunk <= Math.floor((to - 1) / CHUNK_BUCKETS); chunk++)
    loads.push(peaksCache.load(path, version, level, chunk));
  const peaks = gather(await Promise.all(loads), first, from, to, info.channels * 2);
  return { peaks, channels: info.channels, from, rate, key: `${path}#${version}@${level}:${from}-${to}` };
}

/**
 * A file's waveform over its own µs [fromUs, toUs) at peaks level `level` (or its coarsest): null for a file without
 * peaks yet, or that failed to load.
 */
export function loadWave(
  path: string,
  version: number,
  fromUs: number,
  toUs: number,
  level: number,
): Promise<Wave | null> {
  return filePeaks(path, version, fromUs, toUs, level).catch(() => null);
}
