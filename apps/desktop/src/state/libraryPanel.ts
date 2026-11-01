import { create } from 'zustand';

interface LibraryPanelStore {
  /** The library is folded to its rail (`--library-rail`), as chat folds to its strip. */
  collapsed: boolean;
  setCollapsed: (collapsed: boolean) => void;
  toggle: () => void;
}

/** How the library shows: open, or folded to its rail. Control Command S folds it and opens it again. */
export const useLibraryPanel = create<LibraryPanelStore>((set, get) => ({
  collapsed: false,
  setCollapsed: (collapsed) => {
    if (get().collapsed !== collapsed) set({ collapsed });
  },
  toggle: () => set((s) => ({ collapsed: !s.collapsed })),
}));
