import { create } from 'zustand';
import { useCaptionsView, type LeftView } from './captionsView';
import { useSelection } from './selection';

/** A selection as the selection store holds it. */
type Picked = { selectedId: string | null; selectedIds: readonly string[] };

interface LibraryPanelStore {
  /** The library is folded to its rail (`--library-rail`), as chat folds to its strip. */
  collapsed: boolean;
  /**
   * The selection whose card stepped aside for the library (Library in the card's head, a view opened from the card,
   * or Command B): the column shows the library while that selection lasts. Any new pick, even of the same thing,
   * brings the card back (`steppedAside`).
   */
  aside: Picked | null;
  setCollapsed: (collapsed: boolean) => void;
  /**
   * Command B: folds the library to its rail or opens it. While the card shows (`cardShown`, from `cardWanted`), the
   * library folds to its rail and the card steps aside, the selection kept.
   */
  toggle: (cardShown?: boolean) => void;
  /** The card steps aside for the library; the selection stays. */
  stepAside: () => void;
  /** The selection's card comes back (a press on a timeline block, even one already selected). */
  showCard: () => void;
}

/** Whether `aside` is the selection `s`: the very pick it was, so a new pick of the same thing reads as new. */
export function steppedAside(aside: Picked | null, s: Picked): boolean {
  return aside !== null && aside.selectedId === s.selectedId && aside.selectedIds === s.selectedIds;
}

const picked = (): Picked => {
  const { selectedId, selectedIds } = useSelection.getState();
  return { selectedId, selectedIds };
};

/**
 * How the library shows: open, or folded to its rail. Control Command S folds it and opens it again. While something
 * is selected its card shows over the library (`Library`; over the preview's left edge while folded), unless the card
 * stepped aside.
 */
export const useLibraryPanel = create<LibraryPanelStore>((set, get) => ({
  collapsed: false,
  aside: null,
  setCollapsed: (collapsed) => {
    if (get().collapsed !== collapsed) set({ collapsed });
  },
  toggle: (cardShown = false) => {
    if (cardShown) set({ collapsed: true, aside: picked() });
    else set((s) => ({ collapsed: !s.collapsed }));
  },
  stepAside: () => set({ aside: picked() }),
  showCard: () => {
    if (get().aside !== null) set({ aside: null });
  },
}));

/**
 * Shows `view` (the Library, Captions or Looks) in the left column: the card steps aside, the selection kept, and a
 * folded library opens.
 */
export function showLeftView(view: LeftView) {
  useCaptionsView.getState().setView(view);
  useLibraryPanel.getState().stepAside();
  useLibraryPanel.getState().setCollapsed(false);
}
