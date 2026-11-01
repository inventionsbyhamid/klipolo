import { create } from 'zustand';

interface SelectionStore {
  selectedId: string | null;
  select: (id: string | null) => void;
}

export const useSelection = create<SelectionStore>((set) => ({
  selectedId: null,
  select: (selectedId) => set({ selectedId }),
}));

const CAMERA = 'camera:';

/** A recording clip's camera is selected as `camera:<clip id>` (model ids never contain ':'), like the engine's id. */
export function cameraSelection(clipId: string): string {
  return `${CAMERA}${clipId}`;
}

/** The clip whose camera selection `id` names, or null when it names something else. */
export function cameraClipId(id: string | null): string | null {
  return id?.startsWith(CAMERA) ? id.slice(CAMERA.length) : null;
}

const TRANSITION = 'transition:';

/** A clip's transition into the next clip is selected as `transition:<clip id>` (model ids never contain ':'). */
export function transitionSelection(clipId: string): string {
  return `${TRANSITION}${clipId}`;
}

/** The clip whose transition selection `id` names, or null when it names something else. */
export function transitionClipId(id: string | null): string | null {
  return id?.startsWith(TRANSITION) ? id.slice(TRANSITION.length) : null;
}
