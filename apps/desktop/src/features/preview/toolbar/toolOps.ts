import type { Clip } from '../../../bindings/Clip';
import type { ClipPatch } from '../../../bindings/ClipPatch';
import type { Item } from '../../../bindings/Item';
import type { ItemPatch } from '../../../bindings/ItemPatch';
import type { Op } from '../../../bindings/Op';
import type { Label } from '../../../lib/labels';

/** The commit queue's senders live in `state/commit`; re-exported for the tools that import them from here. */
export { commitLatest, commitToggle, queueCommit, type QueuedCommit } from '../../../state/commit';

export const itemOp = (item: Item, patch: ItemPatch): Op => ({ type: 'updateItem', id: item.id, patch });
export const clipOp = (clip: Clip, patch: ClipPatch): Op => ({ type: 'updateClip', id: clip.id, patch });

/** A mute toggle's undo label: `muting` a "sound" is "Mute sound", else "Unmute sound" (`noun` picks the word). */
export const muteLabel = (muting: boolean, noun: 'clip' | 'video' | 'sound'): Label => ({
  id: muting ? 'volume.mute.undo' : 'volume.unmute.undo',
  params: { noun },
});
