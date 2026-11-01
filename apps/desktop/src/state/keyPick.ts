import { create } from 'zustand';
import { deleteKeysAt, deleteVolumeKey, levelNow, type Leveled } from './keyEdits';
import { useSelection } from './selection';
import { itemNow } from './latest';

/**
 * The key picked last on the timeline: a layer's motion keys at one time (a diamond), or a point of a volume line.
 * Delete takes it away rather than its clip or layer, until a press anywhere else lets it go.
 */
export type PickedKey = { kind: 'motion'; itemId: string; tUs: number } | { kind: 'volume'; target: Leveled; tUs: number };

interface KeyPickStore {
  picked: PickedKey | null;
  pick: (key: PickedKey) => void;
  clear: () => void;
}

export const useKeyPick = create<KeyPickStore>((set) => ({
  picked: null,
  pick: (picked) => set({ picked }),
  clear: () => set({ picked: null }),
}));

/** Whether `picked` names this motion key time of layer `itemId`, or this point of `target`'s line. */
export function isPicked(picked: PickedKey | null, owner: string, tUs: number): boolean {
  if (!picked || picked.tUs !== tUs) return false;
  return picked.kind === 'motion' ? picked.itemId === owner : picked.target.id === owner;
}

/** Whether the picked key is still there: a key at its time on its layer, or a point at its time on its line. */
function stillThere(picked: PickedKey): boolean {
  if (picked.kind === 'volume') return levelNow(picked.target).keys.some((k) => k.tUs === picked.tUs);
  const keys = itemNow(picked.itemId)?.layout.motion?.keys;
  return !!keys && Object.values(keys).some((list) => list.some((k) => k.tUs === picked.tUs));
}

/**
 * Delete on the picked key: it goes (one undo step) and is let go. False when no key is picked, or the one picked is
 * gone (deleted by its own Delete, or by an edit): Delete then acts on the selection.
 */
export function deletePickedKey(): boolean {
  const { picked, clear } = useKeyPick.getState();
  if (!picked) return false;
  clear();
  if (!stillThere(picked)) return false;
  if (picked.kind === 'volume') {
    void deleteVolumeKey(picked.target, picked.tUs);
  } else {
    const item = itemNow(picked.itemId);
    if (item) void deleteKeysAt(item, picked.tUs);
  }
  return true;
}

// Another selection (a click, the keys, chat) lets a picked key go.
useSelection.subscribe((s, was) => {
  if (s.selectedId !== was.selectedId && useKeyPick.getState().picked) useKeyPick.getState().clear();
});

// A press anywhere but on a key lets the picked key go: Delete then acts on the selection again.
if (typeof window !== 'undefined')
  window.addEventListener(
    'pointerdown',
    (e) => {
      const on = e.target instanceof Element && e.target.closest('.tl-key, .tl-volume-key');
      if (!on && useKeyPick.getState().picked) useKeyPick.getState().clear();
    },
    true,
  );
