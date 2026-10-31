import type { Asset } from '../../../bindings/Asset';
import { cn } from '../../../design/cn';
import { IMAGE_US } from '../../../lib/placement';
import { useDragState } from '../../../state/dragState';
import { toast } from '../../../state/toasts';
import { dropAutoscroll, dropRefusal, dropTargetAt, setDropIndicator, type DropTarget } from '../dropTarget';
import { DRAG_SLOP_PX, type PointerLike } from './session';

/** The ghost sits this far below and right of the pointer, so it never hides where the drop goes. */
const GHOST_OFFSET_PX = 12;

interface MediaDrag {
  asset: Asset;
  pointerId: number;
  origin: { x: number; y: number };
  last: { x: number; y: number };
  ghost: HTMLElement | null;
  onDrop: (target: DropTarget) => void;
  /** Stops the lanes' edge autoscroll (`dropAutoscroll`), once lifted. */
  stopScroll: (() => void) | null;
}

/** The live media drag: pressed on a media panel row, lifted once past the slop. */
let drag: MediaDrag | null = null;
let frame = 0;

/** How long the media lasts on the timeline, for the drop ghost's width. */
const lengthOf = (asset: Asset) => (asset.kind === 'image' ? IMAGE_US : asset.durationUs);

/**
 * A media panel row's press (a pointer gesture, not HTML5 drag and drop, so it never fights Tauri's file drops). Past
 * 3 px the row's name follows the pointer in a pill, and each frame shows where it would land (`DropIndicator`, and in
 * expanded mode the new-track zone of its kind); near the lanes' left or right edge they scroll. Let go over
 * the timeline, `onDrop` gets that target: only a row that takes the media's kind is one. Escape, a cancelled pointer
 * or a lost capture puts it down with nothing added.
 */
export function beginMediaDrag(e: PointerLike, asset: Asset, onDrop: (target: DropTarget) => void) {
  cancelMediaDrag();
  drag = {
    asset,
    pointerId: e.pointerId,
    origin: { x: e.clientX, y: e.clientY },
    last: { x: e.clientX, y: e.clientY },
    ghost: null,
    onDrop,
    stopScroll: null,
  };
  window.addEventListener('pointermove', onMove);
  window.addEventListener('pointerup', onUp);
  window.addEventListener('pointercancel', onCancel);
  window.addEventListener('lostpointercapture', onCancel);
  window.addEventListener('keydown', onKey, true);
}

function onMove(e: PointerEvent) {
  const d = drag;
  if (!d || e.pointerId !== d.pointerId) return;
  d.last = { x: e.clientX, y: e.clientY };
  if (!d.ghost) {
    if (Math.hypot(d.last.x - d.origin.x, d.last.y - d.origin.y) < DRAG_SLOP_PX) return;
    lift(d);
  }
  frame ||= requestAnimationFrame(draw);
}

function lift(d: MediaDrag) {
  const ghost = document.createElement('div');
  ghost.setAttribute('aria-hidden', 'true');
  ghost.className = cn(
    'pointer-events-none fixed top-0 left-0 z-50 max-w-60 truncate rounded-full bg-raised px-3 py-1 text-[12px] text-fg shadow-popover will-change-transform',
  );
  ghost.textContent = d.asset.name;
  document.body.append(ghost);
  d.ghost = ghost;
  // A picture's or a sound's new-track zone opens (every track showing), as for a dragged layer.
  useDragState.setState({ itemKind: d.asset.kind === 'audio' ? 'audio' : 'overlay' });
  // The lanes scrolled under a still pointer: where it would land changes in the same frame.
  d.stopScroll = dropAutoscroll(
    () => d.last,
    () => {
      if (frame) cancelAnimationFrame(frame);
      draw();
    },
  );
}

/** Once a frame: the ghost to the pointer, and where it would land now. */
function draw() {
  frame = 0;
  const d = drag;
  if (!d?.ghost) return;
  d.ghost.style.transform = `translate(${d.last.x + GHOST_OFFSET_PX}px, ${d.last.y + GHOST_OFFSET_PX}px)`;
  setDropIndicator(dropTargetAt(d.last.x, d.last.y, d.asset.kind), lengthOf(d.asset));
}

function onUp(e: PointerEvent) {
  const d = drag;
  if (!d || e.pointerId !== d.pointerId) return;
  // Where it is let go, not where the last frame drew it.
  const target = d.ghost ? dropTargetAt(e.clientX, e.clientY, d.asset.kind) : null;
  // Let go over a lane that won't take it: which lane does.
  const refused = d.ghost && !target ? dropRefusal(e.clientX, e.clientY, d.asset.kind) : null;
  end();
  if (target) d.onDrop(target);
  else if (refused) toast.info(refused);
}

function onCancel(e: PointerEvent) {
  if (e.pointerId === drag?.pointerId) cancelMediaDrag();
}

function onKey(e: KeyboardEvent) {
  if (e.key !== 'Escape' || !drag?.ghost) return;
  // Escape puts the media down only: it doesn't also clear the selection.
  e.preventDefault();
  e.stopPropagation();
  cancelMediaDrag();
}

/** Puts down the media being dragged, if any: nothing is added. */
export function cancelMediaDrag() {
  if (drag) end();
}

function end() {
  const d = drag;
  drag = null;
  if (frame) cancelAnimationFrame(frame);
  frame = 0;
  window.removeEventListener('pointermove', onMove);
  window.removeEventListener('pointerup', onUp);
  window.removeEventListener('pointercancel', onCancel);
  window.removeEventListener('lostpointercapture', onCancel);
  window.removeEventListener('keydown', onKey, true);
  if (!d?.ghost) return;
  d.stopScroll?.();
  d.ghost.remove();
  setDropIndicator(null);
  useDragState.setState({ itemKind: null });
}
