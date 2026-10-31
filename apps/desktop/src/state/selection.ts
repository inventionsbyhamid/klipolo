import { create } from 'zustand';

interface SelectionStore {
  selectedId: string | null;
  select: (id: string | null) => void;
}

export const useSelection = create<SelectionStore>((set) => ({
  selectedId: null,
  select: (selectedId) => set({ selectedId }),
}));

const TRANSITION = 'transition:';

/** A clip's transition into the next clip is selected as `transition:<clip id>` (model ids never contain ':'). */
export function transitionSelection(clipId: string): string {
  return `${TRANSITION}${clipId}`;
}

/** The clip whose transition selection `id` names, or null when it names something else. */
export function transitionClipId(id: string | null): string | null {
  return id?.startsWith(TRANSITION) ? id.slice(TRANSITION.length) : null;
}
