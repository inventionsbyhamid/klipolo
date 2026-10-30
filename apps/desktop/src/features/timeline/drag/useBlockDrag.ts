import type { PointerEvent } from 'react';
import type { Edge } from '../../../bindings/Edge';
import { beginDrag, cancelDrag, endDrag, moveDrag, type DragView, type Gesture } from './controller';

/** A lane's press handler for its blocks: stable, so `memo` blocks never re-render for it. */
export type PressBlock = (e: PointerEvent<HTMLElement>, id: string) => void;
/** A lane's press handler for its blocks' edges (`TrimHandles`): stable, like `PressBlock`. */
export type PressTrim = (e: PointerEvent<HTMLElement>, id: string, edge: Edge) => void;

/** A press on a trim handle (`[data-trim]`): it trims, so the block's own press leaves it alone. */
export const onTrimHandle = (e: PointerEvent<HTMLElement>) =>
  e.target instanceof Element && e.target.closest('[data-trim]') !== null;

/**
 * A block's press: the primary button only, not on a trim handle. The block captures the pointer, so the gesture
 * keeps its moves and release wherever the pointer goes. `playheadUs`: where the playhead was before the press's pick
 * moved it (the gesture snaps to that).
 */
export function pressBlock(
  e: PointerEvent<HTMLElement>,
  gesture: Gesture,
  view: DragView,
  playheadUs?: number,
) {
  if (e.button !== 0 || onTrimHandle(e)) return;
  e.currentTarget.setPointerCapture?.(e.pointerId);
  beginDrag(e, gesture, view, playheadUs);
}

/** A trim handle's press: its `block`, whose handlers drive every gesture, captures the pointer. */
export function pressTrim(
  e: PointerEvent<HTMLElement>,
  block: HTMLElement,
  gesture: Gesture,
  view: DragView,
) {
  if (e.button !== 0) return;
  block.setPointerCapture?.(e.pointerId);
  beginDrag(e, gesture, view);
}

/** Every block's pointer handlers (stable identities: one controller drives every drag, outside React). */
export const onBlockPointerMove = (e: PointerEvent<HTMLElement>) => moveDrag(e);
export const onBlockPointerUp = (e: PointerEvent<HTMLElement>) => endDrag(e);
export const onBlockPointerCancel = () => cancelDrag();
export const onBlockLostCapture = cancelDrag;
