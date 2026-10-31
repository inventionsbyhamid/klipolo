import { create } from 'zustand';
import type { Clip } from '../bindings/Clip';
import type { EffectInstance } from '../bindings/EffectInstance';
import type { Item } from '../bindings/Item';

export type ClipboardEntry = { kind: 'clip'; clip: Clip } | { kind: 'item'; item: Item };

interface Clipboard {
  /** The project it was copied from: a clip or a layer's media only exists there. */
  projectId: string | null;
  /** A copy of the last block copied or cut (serialised project data, never DOM): ⌘V and ⌘D compile it to Ops. */
  entry: ClipboardEntry | null;
  /** That block's effect stack, for Paste effects; null when it had none. */
  effects: EffectInstance[] | null;
}

/** The timeline's clipboard (`keys/clipboardOps`). */
export const useClipboard = create<Clipboard>(() => ({ projectId: null, entry: null, effects: null }));
