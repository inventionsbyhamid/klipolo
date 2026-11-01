import { create } from 'zustand';

/** A frame of a file, as its filmstrip shows it: the file, and the time in it (µs). */
export interface FileFrame {
  path: string;
  fileUs: number;
}

interface EditFramesStore {
  /**
   * While a roll, slip or slide is dragged, the two frames that meet at the edit: the outgoing clip's last and the
   * incoming clip's first (a slip's: its own first and last). Null otherwise; a side without a filmstrip is null.
   */
  pair: readonly [FileFrame | null, FileFrame | null] | null;
  /** Whether they are a cut's two sides (a roll's, a slide's) or one clip's ends (a slip's). */
  ends: boolean;
}

export const useEditFrames = create<EditFramesStore>(() => ({ pair: null, ends: false }));

/** Shows `pair` beside the preview's frame (`ends`: a slipped clip's first and last), or nothing (null). */
export function showEditFrames(pair: EditFramesStore['pair'], ends = false) {
  const now = useEditFrames.getState();
  const same = (a: FileFrame | null, b: FileFrame | null) =>
    a === b || (a !== null && b !== null && a.path === b.path && a.fileUs === b.fileUs);
  const unchanged = pair === now.pair || (pair && now.pair && same(pair[0], now.pair[0]) && same(pair[1], now.pair[1]));
  if (unchanged && ends === now.ends) return;
  useEditFrames.setState({ pair, ends });
}
