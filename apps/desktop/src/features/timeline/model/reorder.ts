import type { MainIndex } from './index';

/** Where clip `from` lands when the pointer is at `pointerUs`: after every other clip whose middle it passed. */
export function insertionIndex(index: MainIndex, from: number, pointerUs: number): number {
  let k = 0;
  for (let j = 0; j < index.starts.length; j++) {
    if (j !== from && (index.starts[j] + index.ends[j]) / 2 < pointerUs) k++;
  }
  return k;
}

/**
 * The clips between `from` and `to` slide by the dragged clip's length to show the gap it will fill. `within` limits
 * them to clips [first, end) (those that can be on screen), so a clip dragged far costs no more than one dragged near.
 */
export function reorderShifts(
  index: MainIndex,
  from: number,
  to: number,
  within: readonly [number, number] = [0, index.starts.length],
): Map<number, number> {
  const d = index.ends[from] - index.starts[from];
  const [first, end] = within;
  const shifts = new Map<number, number>();
  if (to > from) for (let j = Math.max(from + 1, first); j <= Math.min(to, end - 1); j++) shifts.set(j, -d);
  else for (let j = Math.max(to, first); j < Math.min(from, end); j++) shifts.set(j, d);
  return shifts;
}
