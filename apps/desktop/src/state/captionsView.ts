import { create } from 'zustand';

/** What the left panel shows: the Library, the Captions view or the Looks. */
export type LeftView = 'library' | 'captions' | 'looks';

interface CaptionsView {
  /** The view the left panel shows. This session only. */
  view: LeftView;
  /** The left panel shows the Captions view (`view` is `captions`). */
  open: boolean;
  setView(view: LeftView): void;
  /** Shows the Captions view, or else the Library. */
  setOpen(open: boolean): void;
}

/**
 * Which view the left panel shows: the Library, the Captions view or the Looks. The Captions view
 * widens the panel while it is open and gives back the Library's width when it closes.
 */
export const useCaptionsView = create<CaptionsView>((set, get) => ({
  view: 'library',
  open: false,
  setView: (view) => {
    if (get().view !== view) set({ view, open: view === 'captions' });
  },
  setOpen: (open) => get().setView(open ? 'captions' : 'library'),
}));
