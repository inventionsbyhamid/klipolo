import type { ThumbStrip } from '../../../bindings/ThumbStrip';
import { mediaUrl } from '../../../bridge/commands';
import { media } from '../../../bridge/media';
import { CACHED_SPRITES } from './drawFilmstrip';
import { createLru, once } from './peaksCache';

/**
 * Decoded thumbnail sprites in the webview: at most `CACHED_SPRITES` (16, about 36 MiB). Keyed by the sprite's file,
 * which is written once and never changes.
 */
const images = createLru<HTMLImageElement>(CACHED_SPRITES);
const loading = new Map<string, Promise<HTMLImageElement>>();

/**
 * Sprites decoding at once. Many short clips cut from all over long files each show a frame of their own footage, so a
 * repaint at overview can ask for far more sprites than the cache keeps: they decode a few at a time, the newest
 * request first (what is on screen now), and each is drawn and let go of before many more are decoded.
 */
const DECODES = 3;
/** A decode that hasn't settled by then gives its turn back and fails, so the next paint asks again. */
export const DECODE_TIMEOUT_MS = 10_000;
let decoding = 0;
/**
 * Sprites waiting for a turn, oldest (`head`) to newest (`tail`): the tail goes next, and one asked for again moves to
 * the tail. A doubly linked list with an index by path, so each step is O(1).
 */
interface Waiter {
  path: string;
  go: () => void;
  prev: Waiter | null;
  next: Waiter | null;
}
const waiting = {
  head: null as Waiter | null,
  tail: null as Waiter | null,
  byPath: new Map<string, Waiter>(),
};

function append(w: Waiter) {
  w.prev = waiting.tail;
  w.next = null;
  if (waiting.tail) waiting.tail.next = w;
  else waiting.head = w;
  waiting.tail = w;
}

function unlink(w: Waiter) {
  if (w.prev) w.prev.next = w.next;
  else waiting.head = w.next;
  if (w.next) w.next.prev = w.prev;
  else waiting.tail = w.prev;
  w.prev = w.next = null;
}
/**
 * Whether each paint that asked for a sprite still being loaded is still on (its block still shows that part). A
 * sprite none of them wants is skipped when its turn comes, and one that arrives too late takes only room the cache
 * has spare: it never evicts a sprite an on-screen block draws from.
 */
const askers = new Map<string, (() => boolean)[]>();
/** O(askers of `path`), stopping at the first that still wants it. */
const wanted = (path: string) => askers.get(path)?.some((asks) => asks()) ?? false;

async function inTurn<T>(path: string, work: () => Promise<T>): Promise<T> {
  if (decoding < DECODES) decoding++;
  // A finishing decode hands its turn straight over.
  else
    await new Promise<void>((go) => {
      const w: Waiter = { path, go, prev: null, next: null };
      waiting.byPath.set(path, w);
      append(w);
    });
  try {
    return await work();
  } finally {
    const next = waiting.tail;
    if (next) {
      unlink(next);
      waiting.byPath.delete(next.path);
      next.go();
    } else decoding--;
  }
}

/** `img` decoded, or a failure after `DECODE_TIMEOUT_MS` (a decode that hangs lets go of the file). */
async function decodeWithin(img: HTMLImageElement): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, fail) => {
    timer = setTimeout(() => {
      img.src = '';
      fail(new Error('sprite_decode: timeout'));
    }, DECODE_TIMEOUT_MS);
  });
  try {
    await Promise.race([img.decode(), late]);
  } finally {
    clearTimeout(timer);
  }
}

const always = () => true;

export const spriteCache = {
  image: (path: string) => images.get(path),
  /**
   * The sprite at `path`, decoded off the main thread (`img.decode()`), so drawing it never decodes. `live`: whether
   * the paint asking still wants it (a block scrolled away doesn't).
   */
  load(path: string, live: () => boolean = always): Promise<HTMLImageElement> {
    const hit = images.get(path);
    if (hit) return Promise.resolve(hit);
    const asking = askers.get(path);
    if (asking) asking.push(live);
    else askers.set(path, [live]);
    // Asked for again while it waits: it is on screen now, so it goes next.
    const queued = waiting.byPath.get(path);
    if (queued && queued !== waiting.tail) {
      unlink(queued);
      append(queued);
    }
    return once(loading, path, () =>
      inTurn(path, async () => {
        if (!wanted(path)) throw new Error('sprite_decode: not wanted');
        const img = new Image();
        img.decoding = 'async';
        img.src = mediaUrl(path);
        await decodeWithin(img);
        if (wanted(path) || images.size() < CACHED_SPRITES) images.set(path, img);
        return img;
      }).finally(() => askers.delete(path)),
    );
  },
};

/** A file's thumbnail strip as of media version `version` of it (null until it is scanned). */
const strips = createLru<Promise<ThumbStrip | null>>(64);

export function stripOf(path: string, version: number): Promise<ThumbStrip | null> {
  const key = `${path}|${version}`;
  let strip = strips.get(key);
  if (!strip) {
    strip = media.thumbs(path);
    strips.set(key, strip);
    strip.catch(() => strips.delete(key));
  }
  return strip;
}
