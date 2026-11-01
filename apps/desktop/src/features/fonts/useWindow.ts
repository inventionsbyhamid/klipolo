import { useCallback, useEffect, useRef, useState, type RefObject } from 'react';

/** Every row of a windowed list is this tall (px). */
export const ROW_HEIGHT = 32;
/** Rows drawn above and below the visible ones, so a quick scroll never shows a gap. */
const OVERSCAN = 5;
/** The height assumed before the list is laid out (and where nothing lays out: tests): 10 rows. */
const DEFAULT_HEIGHT = 320;

/**
 * Which of `count` fixed-height rows (`rowHeight` px each) a scrolled list draws (a Mac's 2,000 families
 * never become 2,000 DOM rows; at most about 20 are in the page). It keeps only the first
 * row in view and how many rows fit, nothing per row, so a scroll renders the list again only when that changes, never
 * per scroll event, or when its height changes the rows that fit. `reveal` scrolls
 * a row into view (the keyboard's active row).
 */
export function useWindow(count: number, ref: RefObject<HTMLElement | null>, rowHeight = ROW_HEIGHT) {
  const [view, setView] = useState({ first: 0, visible: Math.ceil(DEFAULT_HEIGHT / rowHeight) });
  const shown = useRef(view);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const read = () => {
      const first = Math.floor(el.scrollTop / rowHeight);
      const visible = Math.ceil((el.clientHeight || DEFAULT_HEIGHT) / rowHeight);
      if (first === shown.current.first && visible === shown.current.visible) return;
      shown.current = { first, visible };
      setView(shown.current);
    };
    read();
    el.addEventListener('scroll', read, { passive: true });
    // A list that fills its panel changes height with the window: the rows that fit
    // follow, scrolled or not.
    const sized = new ResizeObserver(read);
    sized.observe(el);
    return () => {
      el.removeEventListener('scroll', read);
      sized.disconnect();
    };
  }, [ref, rowHeight]);
  const reveal = useCallback(
    (index: number) => {
      const el = ref.current;
      if (!el) return;
      const height = el.clientHeight || DEFAULT_HEIGHT;
      const top = index * rowHeight;
      if (top < el.scrollTop) el.scrollTop = top;
      else if (top + rowHeight > el.scrollTop + height) el.scrollTop = top + rowHeight - height;
    },
    [ref, rowHeight],
  );
  return {
    start: Math.max(0, view.first - OVERSCAN),
    end: Math.min(count, view.first + view.visible + OVERSCAN),
    reveal,
  };
}
