import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { Browse } from '../../bindings/Browse';
import type { BrowseAsk } from '../../bindings/BrowseAsk';
import type { FontRow } from '../../bindings/FontRow';
import { fonts } from '../../bridge/fonts';

/** Rows asked for at once: a window is about 20, so a page covers a scroll of a few windows. */
const PAGE = 64;
/** Pages kept either side of the window's: farther ones are let go, and asked again when scrolled back to. */
const NEAR = 1;

/**
 * The font menu's browse (an empty query), windowed: Rust's sections and counts, then only the rows in view, a page
 * at a time (`want`), so the page never holds a Mac's thousands of families and Google's 1,900 at once: only the
 * pages near the window are kept, and none while the menu is closed. Asked again when the ask changes, and when fonts
 * arrive (`fonts-scanned`) or change (`fonts-changed`). Null until the first answer.
 */
export function useBrowse(ask: BrowseAsk, on: boolean) {
  const [browse, setBrowse] = useState<Browse | null>(null);
  const [rows, setRows] = useState<Map<number, FontRow>>(() => new Map());
  const [changes, setChanges] = useState(0);
  const key = JSON.stringify(ask);
  // The pages asked for this answer (by first row), so a scroll never asks one twice.
  const asked = useRef<{ key: string; pages: Set<number> }>({ key: '', pages: new Set() });
  const serial = useRef(0);
  useEffect(() => {
    if (!on) return;
    let stop: (() => void)[] = [];
    let live = true;
    const again = () => setChanges((n) => n + 1);
    for (const event of ['fonts-scanned', 'fonts-changed'])
      listen(event, again)
        .then((unlisten) => (live ? stop.push(unlisten) : unlisten()))
        .catch(() => undefined);
    return () => {
      live = false;
      stop.forEach((f) => f());
      stop = [];
    };
  }, [on]);
  useEffect(() => {
    if (!on) {
      // Closed: nothing held, and a late page is never kept.
      serial.current += 1;
      asked.current = { key: '', pages: new Set() };
      setRows((held) => (held.size === 0 ? held : new Map()));
      return;
    }
    const mine = ++serial.current;
    const now = `${key}#${changes}`;
    asked.current = { key: now, pages: new Set() };
    fonts
      .browse(JSON.parse(key) as BrowseAsk)
      .then((b) => {
        if (mine !== serial.current) return;
        setRows(new Map());
        setBrowse(b);
      })
      .catch(() => {
        if (mine === serial.current) setBrowse({ sections: [], value: null });
      });
  }, [key, on, changes]);
  /** Rows `from..to` are in view: their pages are asked for, once each. */
  const want = useCallback(
    (from: number, to: number) => {
      if (!on || !browse || to <= from) return;
      const now = asked.current;
      const mine = serial.current;
      const lo = (Math.floor(from / PAGE) - NEAR) * PAGE;
      const hi = (Math.ceil(to / PAGE) + NEAR) * PAGE;
      const far = [...now.pages].filter((page) => page < lo || page >= hi);
      if (far.length > 0) {
        far.forEach((page) => now.pages.delete(page));
        setRows((held) => {
          const next = new Map<number, FontRow>();
          held.forEach((r, i) => i >= lo && i < hi && next.set(i, r));
          return next;
        });
      }
      for (let page = Math.floor(from / PAGE) * PAGE; page < to; page += PAGE) {
        if (now.pages.has(page)) continue;
        now.pages.add(page);
        fonts
          .browseRows(JSON.parse(key) as BrowseAsk, page, page + PAGE)
          .then((got) => {
            // A page let go (scrolled far from) before it came isn't kept.
            if (mine !== serial.current || !now.pages.has(page)) return;
            setRows((held) => {
              const next = new Map(held);
              got.forEach((r, i) => next.set(page + i, r));
              return next;
            });
          })
          .catch(() => now.pages.delete(page));
      }
    },
    [browse, key, on],
  );
  return { browse, rows, want };
}
