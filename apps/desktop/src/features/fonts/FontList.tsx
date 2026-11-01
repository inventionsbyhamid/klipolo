import { useCallback, useEffect, useLayoutEffect, useRef, type RefObject } from 'react';
import { FontRow, PendingRow } from './FontRow';
import type { Listing } from './listing';
import { ROW_HEIGHT, useWindow } from './useWindow';

export type { ListItem } from './listing';

/**
 * The font menu's list (`role="listbox"`), windowed: only the lines in view (and a few beyond) are in the page, each
 * 32 px tall, and a browse's rows arrive for those lines only (`onWindow`). Section headers are
 * `role="presentation"`. The search field owns the focus and points at the active option (`aria-activedescendant`,
 * `optionId`), so typing and arrow keys never leave it.
 */
export function FontList({
  listing,
  idPrefix,
  active,
  activeItem,
  label,
  describedBy,
  onPick,
  onHover,
  onWindow,
  listRef,
}: {
  listing: Listing;
  idPrefix: string;
  /** The active option's number, or -1. */
  active: number;
  /** Its line (kept in view), or -1. */
  activeItem: number;
  label: string;
  /** The id of a line that says more of the list (Google Fonts is off, a download's progress). */
  describedBy?: string;
  onPick: (option: number) => void;
  onHover: (option: number) => void;
  /** The options on the lines in view now (`[from, to)`). */
  onWindow?: (from: number, to: number) => void;
  listRef: RefObject<HTMLDivElement | null>;
}) {
  const { start, end, reveal } = useWindow(listing.lines, listRef);
  // Stable for the memoised rows: each calls them with its own option's number.
  const latest = useRef({ onPick, onHover });
  useLayoutEffect(() => {
    latest.current = { onPick, onHover };
  });
  const pick = useCallback((n: number) => latest.current.onPick(n), []);
  const hover = useCallback((n: number) => latest.current.onHover(n), []);
  useEffect(() => {
    if (activeItem >= 0) reveal(activeItem);
  }, [activeItem, reveal]);
  useEffect(() => {
    if (end > start) onWindow?.(...listing.optionsIn(start, end - 1));
  }, [start, end, listing, onWindow]);
  const lines = [];
  for (let i = start; i < end; i++) lines.push(i);
  return (
    <div
      ref={listRef}
      id={`${idPrefix}-list`}
      role="listbox"
      aria-label={label}
      aria-describedby={describedBy}
      className="font-list"
    >
      <div className="font-list-body" style={{ height: listing.lines * ROW_HEIGHT }}>
        {lines.map((i) => {
          const item = listing.line(i);
          const top = i * ROW_HEIGHT;
          if ('header' in item)
            return (
              <div
                key={`h${i}`}
                role="presentation"
                style={{ top, height: ROW_HEIGHT }}
                className="font-list-header"
              >
                {item.header}
              </div>
            );
          const n = item.option;
          const row = listing.row(n);
          return row ? (
            <FontRow
              key={`o${n}`}
              n={n}
              id={optionId(idPrefix, n)}
              row={row}
              posinset={n + 1}
              setsize={listing.options}
              active={n === active}
              top={top}
              onPick={pick}
              onHover={hover}
            />
          ) : (
            <PendingRow
              key={`o${n}`}
              id={optionId(idPrefix, n)}
              position={[n + 1, listing.options]}
              active={n === active}
              top={top}
            />
          );
        })}
      </div>
    </div>
  );
}

/** The DOM id of option `n` in the list `idPrefix` names. */
export const optionId = (idPrefix: string, n: number) => `${idPrefix}-o${n}`;
