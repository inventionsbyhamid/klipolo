import { listen } from '@tauri-apps/api/event';
import { useEffect, useRef, useState } from 'react';
import type { FontRef } from '../../bindings/FontRef';
import type { FontRow } from '../../bindings/FontRow';
import type { SearchFilter } from '../../bindings/SearchFilter';
import { fonts } from '../../bridge/fonts';

/** Moves on when the fonts change: this computer's arrived (`fonts-scanned`), or a download or removal. */
function useFontsChanges(on: boolean): number {
  const [changes, setChanges] = useState(0);
  useEffect(() => {
    if (!on) return;
    const stop: (() => void)[] = [];
    let live = true;
    for (const event of ['fonts-scanned', 'fonts-changed'])
      listen(event, () => setChanges((n) => n + 1))
        .then((unlisten) => (live ? stop.push(unlisten) : unlisten()))
        .catch(() => undefined);
    return () => {
      live = false;
      stop.forEach((f) => f());
    };
  }, [on]);
  return changes;
}

/**
 * The families matching `query` and `filter`, from Rust's one ranked search (`fonts_search`): asked on every change
 * while `on`, the latest answer winning (an earlier one arriving late is dropped), and again when the fonts change.
 * Each row carries the weight a pick draws at for `filter.want`. Null until the first answer.
 */
export function useFontSearch(query: string, filter: SearchFilter, on: boolean): FontRow[] | null {
  const [rows, setRows] = useState<FontRow[] | null>(null);
  const changes = useFontsChanges(on);
  const asked = useRef(0);
  const key = JSON.stringify(filter);
  useEffect(() => {
    if (!on) return;
    const ask = ++asked.current;
    fonts
      .search(query, JSON.parse(key) as SearchFilter)
      .then((found) => {
        if (ask === asked.current) setRows(found);
      })
      .catch(() => {
        if (ask === asked.current) setRows([]);
      });
  }, [query, key, on, changes]);
  return rows;
}

/**
 * The row of exactly `font` (else a family of its name), for the menu's trigger and its Weight menu: one lookup in
 * Rust (`fonts_row`), not a search. Asked again when the fonts change.
 */
export function useFontRow(font: FontRef, weight: number, italic: boolean): FontRow | null {
  return useFontRowAnswer(font, weight, italic) ?? null;
}

/** `useFontRow`'s answer, undefined until Rust has given one (null: no such family here or at Google). */
export function useFontRowAnswer(font: FontRef, weight: number, italic: boolean): FontRow | null | undefined {
  const [answer, setAnswer] = useState<{ key: string; row: FontRow | null } | null>(null);
  const changes = useFontsChanges(true);
  const key = JSON.stringify([font, weight, italic]);
  const asked = useRef(0);
  useEffect(() => {
    const ask = ++asked.current;
    const [f, w, i] = JSON.parse(key) as [FontRef, number, boolean];
    fonts
      .row(f, w, i)
      .then((found) => {
        if (ask === asked.current) setAnswer({ key, row: found ?? null });
      })
      .catch(() => {
        if (ask === asked.current) setAnswer({ key, row: null });
      });
  }, [key, changes]);
  // An answer for another font is none for this one: never a flash of the last font's row.
  return answer && answer.key === key ? answer.row : undefined;
}
