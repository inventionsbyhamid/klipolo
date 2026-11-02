import { useEffect, useState } from 'react';

/** The rows a long library tab draws at once. */
export const FIRST_ROWS = 40;
/** The rows it adds each frame after them. */
export const ROWS_A_FRAME = 25;

const later = (cb: () => void) =>
  typeof requestAnimationFrame === 'function'
    ? { cancel: cancelAnimationFrame.bind(null, requestAnimationFrame(cb)) }
    : { cancel: clearTimeout.bind(null, setTimeout(cb, 16)) };

/**
 * How many of a tab's `total` rows to draw (Infinity: all): its first `FIRST_ROWS` as the tab (or its search, `key`)
 * changes, then `ROWS_A_FRAME` more each frame until every row is drawn, so a tab of hundreds of files never holds the
 * page for one long frame. Drawn again for anything else (an edit), the rows drawn stay.
 */
export function useRowBudget(key: string, total: number): number {
  const [budget, setBudget] = useState({ key, rows: FIRST_ROWS });
  const rows = budget.key === key ? budget.rows : FIRST_ROWS;
  useEffect(() => {
    if (rows >= total) return;
    const next = later(() => setBudget({ key, rows: rows + ROWS_A_FRAME }));
    return next.cancel;
  }, [key, rows, total]);
  return rows >= total ? Infinity : rows;
}
