import type { Clip } from '../bindings/Clip';
import type { ClipPatch } from '../bindings/ClipPatch';
import type { Item } from '../bindings/Item';
import type { ItemPatch } from '../bindings/ItemPatch';
import type { Op } from '../bindings/Op';

/** The op that changes `patch`'s fields of layer `item`. */
export const itemOp = (item: Item, patch: ItemPatch): Op => ({ type: 'updateItem', id: item.id, patch });
/** The op that changes `patch`'s fields of clip `clip`. */
export const clipOp = (clip: Clip, patch: ClipPatch): Op => ({ type: 'updateClip', id: clip.id, patch });
