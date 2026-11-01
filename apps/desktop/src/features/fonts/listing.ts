import { useLingui } from '@lingui/react/macro';
import { useMemo } from 'react';
import type { Browse } from '../../bindings/Browse';
import type { FontRow as FontRowData } from '../../bindings/FontRow';
import type { Section } from '../../bindings/Section';

/** One line of the list: a section's header, or a family (its option's number). */
export type ListItem = { header: string } | { option: number };

/**
 * The font menu's lines, however they came: a typed query's rows, all at hand, or a browse's sections
 * whose rows arrive by window (`useBrowse`). Lines are headers and options; nothing per line is built until drawn.
 */
export interface Listing {
  /** Lines: headers and options. */
  lines: number;
  options: number;
  line(i: number): ListItem;
  /** Option `n`'s row, once it is here (a browse's arrive by window). */
  row(n: number): FontRowData | undefined;
  /** The line option `n` is on. */
  lineOf(n: number): number;
  /** The options on lines `from..to`. */
  optionsIn(from: number, to: number): [number, number];
}

/** Each section's header. */
export function useHeaders(): Record<Section, string> {
  const { t } = useLingui();
  // One object per language, so the listings built from it are memoised.
  return useMemo(
    () => ({
      project: t({ id: 'fontMenu.inProject', message: 'In this project' }),
      recent: t({ id: 'fontMenu.recent', message: 'Recent' }),
      bundled: t({
        id: 'fontMenu.bundled',
        message: 'Included with Klipolo',
        comment: 'Fonts that come with the app',
      }),
      installed: t({
        id: 'fontMenu.installed',
        message: 'On this Mac',
        comment: 'Fonts installed on this computer',
      }),
      google: t({
        id: 'fontMenu.google',
        message: 'Google Fonts',
        comment: 'Free fonts from Google, downloaded when picked',
      }),
    }),
    [t],
  );
}

/** A listing of sections of known size, `rows` holding the options that have arrived. */
export function sectionsListing(
  sections: [string, number][],
  rows: (n: number) => FontRowData | undefined,
): Listing {
  // Per section: its first line and its first option.
  const starts: { line: number; option: number; count: number; header: string }[] = [];
  let line = 0;
  let option = 0;
  for (const [header, count] of sections) {
    if (count <= 0) continue;
    starts.push({ line, option, count, header });
    line += count + 1;
    option += count;
  }
  const sectionOfLine = (i: number) => {
    let lo = 0;
    for (let s = 0; s < starts.length; s++) if (starts[s].line <= i) lo = s;
    return starts[lo];
  };
  const sectionOfOption = (n: number) => {
    let lo = 0;
    for (let s = 0; s < starts.length; s++) if (starts[s].option <= n) lo = s;
    return starts[lo];
  };
  const optionAt = (i: number) => {
    const s = sectionOfLine(i);
    return s.option + Math.max(0, i - s.line - 1);
  };
  return {
    lines: line,
    options: option,
    line: (i) => {
      const s = sectionOfLine(i);
      return i === s.line ? { header: s.header } : { option: s.option + (i - s.line - 1) };
    },
    row: rows,
    lineOf: (n) => {
      const s = sectionOfOption(n);
      return s.line + 1 + (n - s.option);
    },
    optionsIn: (from, to) =>
      starts.length === 0 ? [0, 0] : [optionAt(from), Math.min(option, optionAt(to) + 1)],
  };
}

/** A browse's listing (`Browse` from Rust) over the rows that have arrived. */
export function useBrowseListing(browse: Browse | null, rows: Map<number, FontRowData>): Listing {
  const headers = useHeaders();
  return useMemo(
    () =>
      sectionsListing(
        (browse?.sections ?? []).map(([s, n]) => [headers[s], n]),
        (n) => rows.get(n),
      ),
    [browse, rows, headers],
  );
}

/**
 * A typed query's listing: Rust's rows in sections, "Included with Klipolo", "On this Mac", then "Google Fonts" (its
 * families downloaded here or not). Memoised on the answer: hovering and the arrow keys build
 * nothing again.
 */
export function useQueryListing(rows: FontRowData[]): Listing {
  const headers = useHeaders();
  return useMemo(() => {
    const parts: [string, FontRowData[]][] = [
      [headers.bundled, rows.filter((r) => r.state === 'bundled')],
      [headers.installed, rows.filter((r) => r.state === 'installed')],
      [headers.google, rows.filter((r) => r.source.type === 'google')],
    ];
    const all = parts.flatMap(([, list]) => list);
    return sectionsListing(
      parts.map(([h, list]) => [h, list.length]),
      (n) => all[n],
    );
  }, [rows, headers]);
}
