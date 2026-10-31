import { create } from 'zustand';

interface LayoutStore {
  /** A separator is pressed (a drag may be under way). */
  resizing: boolean;
  /** The panel group's width (px), as laid out now; 0 until it is measured. A small window folds panels by it. */
  groupPx: number;
  setResizing(on: boolean): void;
  setGroupPx(px: number): void;
}

/** Layout state that changes per frame; never persisted (zustand's persist writes on every set). */
export const useLayout = create<LayoutStore>()((set, get) => ({
  resizing: false,
  groupPx: 0,
  setResizing: (on) => {
    if (get().resizing !== on) set({ resizing: on });
  },
  setGroupPx: (px) => {
    if (get().groupPx !== px) set({ groupPx: px });
  },
}));
