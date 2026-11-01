import { create } from 'zustand';

interface SelectionStore {
  /**
   * The block picked last: the one the cards, the preview and the keys act on while it is the only one, and the one a
   * ⇧-click's range starts from.
   */
  selectedId: string | null;
  /**
   * Every selected clip and layer, in the order they were picked, `selectedId` among them; more than one is a group
   * (`selectionOf`). Only clips and layers group: a camera or a transition is always selected alone.
   */
  selectedIds: readonly string[];
  /** Selects `id` alone, or nothing. */
  select: (id: string | null) => void;
  /** ⌘-click: adds `id` to the selection, or takes it out (the one picked before it is then the last). */
  toggle: (id: string) => void;
  /** Selects `ids` (a ⇧-click's range, a box, ⌘A); `last` (else the last of them) is the one picked last. */
  selectMany: (ids: readonly string[], last?: string | null) => void;
}

const NONE: readonly string[] = [];
/** A camera's or a transition's selection (`camera:<clip>`, `transition:<clip>`), which never groups. */
const isPart = (id: string) => id.includes(':');

export const useSelection = create<SelectionStore>((set, get) => ({
  selectedId: null,
  selectedIds: NONE,
  select: (selectedId) => set({ selectedId, selectedIds: selectedId === null ? NONE : [selectedId] }),
  toggle: (id) => {
    if (isPart(id)) return get().select(id);
    const ids = selectionOf(get()).filter((other) => !isPart(other));
    if (!ids.includes(id)) return set({ selectedId: id, selectedIds: [...ids, id] });
    const kept = ids.filter((other) => other !== id);
    set({ selectedId: kept.at(-1) ?? null, selectedIds: kept.length === 0 ? NONE : kept });
  },
  selectMany: (ids, last) => {
    const unique = [...new Set(ids)];
    const primary = last !== undefined && last !== null && unique.includes(last) ? last : (unique.at(-1) ?? null);
    set({ selectedId: primary, selectedIds: unique.length === 0 ? NONE : unique });
  },
}));

/**
 * What is selected, as a list: the group, or the one block (`selectedId`) when a change set it alone. A store set only
 * its `selectedId` (a test, an older caller) reads as that one block.
 */
export function selectionOf(s: Pick<SelectionStore, 'selectedId' | 'selectedIds'>): readonly string[] {
  if (s.selectedId === null) return NONE;
  return s.selectedIds.length > 1 && s.selectedIds.includes(s.selectedId) ? s.selectedIds : [s.selectedId];
}

/** The selected group's ids, or null when one block or nothing is selected. */
export function groupOf(s: Pick<SelectionStore, 'selectedId' | 'selectedIds'>): readonly string[] | null {
  const ids = selectionOf(s);
  return ids.length > 1 ? ids : null;
}

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
