import { create } from 'zustand';
import { deleteKeysAt, deleteVolumeKey, type Leveled } from './keyEdits';
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

/** Delete on the picked key: it goes (one undo step) and is let go. False when no key is picked. */
export function deletePickedKey(): boolean {
  const { picked, clear } = useKeyPick.getState();
  if (!picked) return false;
  clear();
  if (picked.kind === 'volume') {
    void deleteVolumeKey(picked.target, picked.tUs);
  } else {
    const item = itemNow(picked.itemId);
    if (item) void deleteKeysAt(item, picked.tUs);
  }
  return true;
}

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
