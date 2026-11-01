import { create } from 'zustand';
import type { Mark } from '../../bindings/Mark';
import { library, type LibraryMarks } from '../../bridge/library';
import { errorMessage } from '../../lib/errors';
import { toast } from '../../state/toasts';

/*
 * The library's favorites, for every project (`crates/klipolo-app/src/library_marks.rs`): a star shows at once and is
 * saved app-wide. Each list on a tab shows its favorites first, then the rest in its own order.
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

/** `items` as a list on a tab shows them: favorites first, then the rest, each in `items`' order. */
export function marked<T>(items: T[], keyOf: (item: T) => string, marks: LibraryMarks): T[] {
  const rank = (item: T) => (marks[keyOf(item)]?.favorite ? 0 : 1);
  return items
    .map((item, index) => ({ item, index, rank: rank(item) }))
    .sort((a, b) => a.rank - b.rank || a.index - b.index)
    .map((row) => row.item);
}
