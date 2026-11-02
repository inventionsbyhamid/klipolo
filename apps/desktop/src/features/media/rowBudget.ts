import { useDeferredValue } from 'react';

/** The rows a long library tab draws at once. */
export const FIRST_ROWS = 40;

/**
 * How many of a tab's rows to draw: its first `FIRST_ROWS` as the tab (or its search, `key`) changes, then every row in
 * the background render React does next (`useDeferredValue`), which gives way to the person's input. A tab of hundreds
 * of files never holds the page while it draws; drawn again for anything else (an edit), every row at once.
 */
export function useRowBudget(key: string): number {
  const settled = useDeferredValue(key, '');
  return settled === key ? Infinity : FIRST_ROWS;
}
