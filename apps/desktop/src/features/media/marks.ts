import { create } from 'zustand';
import type { Mark } from '../../bindings/Mark';
import { library, type LibraryMarks } from '../../bridge/library';
import { errorMessage } from '../../lib/errors';
import { toast } from '../../state/toasts';

/*
 * The library's favorites and recently used items, for every project (`crates/klipolo-app/src/library_marks.rs`): a
 * star shows at once and is saved app-wide; an item counts as used when the library adds it to the timeline. Each
 * list on a tab shows its favorites first, then what was used most recently, then the rest in its own order.
 */

/** A pack item's key, the same in every project. */
export const packKey = (id: string) => `pack:${id}`;
/** A file's key: its path, the same in every project that has it. */
export const fileKey = (path: string) => `file:${path}`;

export const useMarks = create<{ marks: LibraryMarks; asked: boolean }>(() => ({ marks: {}, asked: false }));

/** The marks as Rust keeps them, read once per page (asked again by the next library tab when they couldn't be). */
export function loadMarks() {
  if (useMarks.getState().asked) return;
  useMarks.setState({ asked: true });
  void library
    .marks()
    .then((marks) => useMarks.setState({ marks }))
    .catch(() => useMarks.setState({ asked: false }));
}

/** `key`'s mark as `mark` (none: unmarked). */
function put(key: string, mark: Mark | undefined) {
  useMarks.setState((s) => {
    const marks = { ...s.marks };
    if (mark && (mark.favorite || mark.usedMs > 0)) marks[key] = mark;
    else delete marks[key];
    return { marks };
  });
}

/** Stars item `key` (`on`) or takes its star off: shown at once, saved app-wide, taken back with a toast if it can't be. */
export function setFavorite(key: string, on: boolean) {
  const was = useMarks.getState().marks[key];
  put(key, { favorite: on, usedMs: was?.usedMs ?? 0 });
  library
    .setFavorite(key, on)
    .then((saved) => put(key, saved[key]))
    .catch((error: unknown) => {
      put(key, { favorite: was?.favorite ?? false, usedMs: useMarks.getState().marks[key]?.usedMs ?? 0 });
      toast.error(errorMessage(error));
    });
}

/** Item `key` was added to the timeline from the library just now: listed first after the favorites. */
export function markUsed(key: string) {
  put(key, { favorite: useMarks.getState().marks[key]?.favorite ?? false, usedMs: Date.now() });
  // Not kept for the next project when it can't be saved; this one still lists it first.
  library
    .markUsed(key)
    .then((saved) => put(key, saved[key]))
    .catch(() => undefined);
}

/** Resolves with `added` once it is known, noting item `key` as used when it went in. */
export function markWhenAdded(key: string, added: Promise<boolean>): Promise<boolean> {
  return added.then((ok) => {
    if (ok) markUsed(key);
    return ok;
  });
}

/**
 * `items` as a list on a tab shows them: favorites first, then those used, most recently first, then the rest, each
 * group in `items`' order.
 */
export function marked<T>(items: T[], keyOf: (item: T) => string, marks: LibraryMarks): T[] {
  const rank = (item: T) => {
    const mark = marks[keyOf(item)];
    return mark?.favorite ? [0, 0] : mark && mark.usedMs > 0 ? [1, -mark.usedMs] : [2, 0];
  };
  return items
    .map((item, index) => ({ item, index, rank: rank(item) }))
    .sort((a, b) => a.rank[0] - b.rank[0] || a.rank[1] - b.rank[1] || a.index - b.index)
    .map((row) => row.item);
}
