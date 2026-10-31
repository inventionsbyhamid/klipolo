import { create } from 'zustand';

interface CaptionsView {
  /** The left panel shows the Captions view (else the Library). This session only. */
  open: boolean;
  setOpen(open: boolean): void;
}

/**
 * Which view the left panel shows: the Library, the Captions view or the Looks. The Captions view
 * panel while it is open and gives back the Library's width when it closes.
 */
export const useCaptionsView = create<CaptionsView>((set, get) => ({
  open: false,
  setOpen: (open) => {
    if (get().open !== open) set({ open });
  },
}));
