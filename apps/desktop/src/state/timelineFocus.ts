import { create } from 'zustand';

interface TimelineFocus {
  /** The one block in the Tab order (roving tabindex): the one last focused or selected. */
  activeId: string | null;
  /** Set when the keyboard moved focus: the view focuses that block once it has rendered. */
  pending: boolean;
  /**
   * Counts the keyboard's moves, so the view renders for each one (even to the block already in the Tab order) and
   * not again when it has moved focus (`settle` leaves it alone).
   */
  moves: number;
  /** `id` becomes the block in the Tab order; `fromKeyboard`, the view also moves focus to it. */
  focus: (id: string | null, fromKeyboard: boolean) => void;
  /** The view has moved focus. */
  settle: () => void;
}

/**
 * Which timeline block takes focus: read by the view, which draws it tabbable, keeps it mounted and focuses it.
 */
export const useTimelineFocus = create<TimelineFocus>((set, get) => ({
  activeId: null,
  pending: false,
  moves: 0,
  focus: (activeId, fromKeyboard) => {
    const now = get();
    if (fromKeyboard) return set({ activeId, pending: true, moves: now.moves + 1 });
    // A focus still to be moved there stays to be moved.
    const pending = activeId === now.activeId && now.pending;
    if (activeId !== now.activeId || pending !== now.pending) set({ activeId, pending });
  },
  settle: () => {
    if (get().pending) set({ pending: false });
  },
}));
