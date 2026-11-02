import { create } from 'zustand';
import { deleteKeysAt, deleteVolumeKey, type Leveled } from './keyEdits';
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

/**
 * Delete on the picked key: it goes (one undo step, at its turn, so after a move of it still on its way) and is let go.
 * A key that is gone by then stays gone and nothing else changes: the Delete is spent on the pick all the same, so a
 * clip or layer never goes in a key's place. False only when no key is picked (its own Delete lets it go): Delete then
 * acts on the selection.
 */
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

/**
 * A key dragged to `key.tUs` from `fromUs`: the pick goes with it at once (a key is a new button at its new time, so
 * its own Delete is gone with the old one), and once the move lands, to where it went, or back to `fromUs` when it
 * didn't go. A pick made or let go meanwhile is kept.
 */
export function pickMoved(key: PickedKey, fromUs: number, landed: Promise<number | null>) {
  useKeyPick.getState().pick(key);
  void landed.then((at) => {
    const tUs = at ?? fromUs;
    if (useKeyPick.getState().picked === key && tUs !== key.tUs) useKeyPick.getState().pick({ ...key, tUs });
  });
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
