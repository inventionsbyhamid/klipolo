import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';

/** Lines are drawn in chunks of this many: a scroll draws again only as it reaches another chunk. */
export const CHUNK = 50;
/** A line's height until it is measured (the lines' `contain-intrinsic-size`). */
const LINE_PX = 52;
/** How far past the view, each way, lines are drawn (px). */
const OVERSCAN_PX = 800;

/** The chunks drawn, by index; the rest of the list is gaps of their height. */
export interface LineWindow {
  drawn: Set<number>;
  /** A chunk's height: measured once drawn, or estimated from the lines measured so far. */
  heightOf: (chunk: number) => number;
}

const later = (cb: () => void) =>
  typeof requestAnimationFrame === 'function' ? requestAnimationFrame(cb) : setTimeout(cb, 16);

const same = (a: Set<number>, b: Set<number>) => a.size === b.size && [...a].every((c) => b.has(c));

/**
 * Which chunks of a long list of `count` lines to draw: those within `OVERSCAN_PX` of what its scroll box (`.cap-scroll`)
 * shows, and always the chunks holding the `pinned` lines (the active word's, the one being fixed, the one now
 * playing), so the list can point at them and bring them into view. Without a layout (a folded panel), the chunks
 * drawn stay as they were: the first one, and the pinned ones. `lineAt(k)` is the element of the list's `k`th line,
 * when drawn, for measuring; `reset` changes when the lines do (a search), which forgets their heights.
 */
export function useLineWindow(
  listRef: RefObject<HTMLElement | null>,
  count: number,
  pinned: number[],
  lineAt: (k: number) => HTMLElement | null,
  reset: unknown,
): LineWindow {
  const chunks = Math.ceil(count / CHUNK);
  const heights = useRef(new Map<number, number>());
  const [seen, setSeen] = useState<Set<number>>(() => new Set([0]));
  const lastReset = useRef(reset);
  if (lastReset.current !== reset) {
    lastReset.current = reset;
    heights.current = new Map();
  }

  const lineHeight = () => {
    let px = 0;
    let lines = 0;
    for (const [c, h] of heights.current) {
      px += h;
      lines += Math.min(CHUNK, count - c * CHUNK);
    }
    return lines > 0 ? px / lines : LINE_PX;
  };
  const heightOf = (c: number) => heights.current.get(c) ?? lineHeight() * Math.min(CHUNK, count - c * CHUNK);

  /** The chunks within reach of the view now; null without a layout. */
  const inView = useCallback((): Set<number> | null => {
    const list = listRef.current;
    const box = list?.closest<HTMLElement>('.cap-scroll');
    if (!list || !box || box.clientHeight === 0) return null;
    const listTop = list.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop;
    const from = box.scrollTop - listTop - OVERSCAN_PX;
    const to = box.scrollTop - listTop + box.clientHeight + OVERSCAN_PX;
    const out = new Set<number>();
    let y = 0;
    for (let c = 0; c < chunks; c++) {
      const h = heightOf(c);
      if (y + h >= from && y <= to) out.add(c);
      if (y > to) break;
      y += h;
    }
    return out;
    // `heightOf` reads the heights from a ref: the list and its length are what change it.
  }, [listRef, chunks, count]);

  const update = useCallback(() => {
    const next = inView();
    if (next) setSeen((was) => (same(was, next) ? was : next));
  }, [inView]);

  // Scrolls and resizes of the box move the window, at most once a frame.
  useEffect(() => {
    const box = listRef.current?.closest<HTMLElement>('.cap-scroll');
    if (!box) return;
    let queued = false;
    const onScroll = () => {
      if (queued) return;
      queued = true;
      later(() => {
        queued = false;
        update();
      });
    };
    box.addEventListener('scroll', onScroll, { passive: true });
    const resize = typeof ResizeObserver === 'function' ? new ResizeObserver(onScroll) : null;
    resize?.observe(box);
    return () => {
      box.removeEventListener('scroll', onScroll);
      resize?.disconnect();
    };
  }, [listRef, update]);

  const drawn = new Set([...seen].filter((c) => c < chunks));
  for (const k of pinned) if (k >= 0 && k < count) drawn.add(Math.floor(k / CHUNK));
  if (drawn.size === 0 && chunks > 0) drawn.add(0);

  // Each drawn chunk's height, once on screen; a change moves the window to match (and the list's gaps with it).
  useLayoutEffect(() => {
    let changed = false;
    for (const c of drawn) {
      const first = lineAt(c * CHUNK);
      const last = lineAt(Math.min(count, (c + 1) * CHUNK) - 1);
      if (!first || !last) continue;
      const h = last.offsetTop + last.offsetHeight - first.offsetTop;
      if (h > 0 && Math.abs((heights.current.get(c) ?? 0) - h) > 1) {
        heights.current.set(c, h);
        changed = true;
      }
    }
    if (changed) update();
  });
  // A new list (a search, new pages): the window as it is now.
  useLayoutEffect(update, [update, reset]);

  return { drawn, heightOf };
}
