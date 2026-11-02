import { t } from '@lingui/core/macro';
import { memo, useCallback, useRef, type KeyboardEvent, type PointerEvent } from 'react';
import type { Motion } from '../../../bindings/Motion';
import { motionKeyTimes } from '../../../lib/keys';
import { formatPrecise } from '../../../lib/time';
import { deleteKeysAt, moveKeysAt } from '../../../state/keyEdits';
import { itemNow } from '../../../state/latest';
import { isPicked, useKeyPick } from '../../../state/keyPick';
import { usePlayback } from '../../../state/playback';
import { useSelection } from '../../../state/selection';
import { registerFollower } from '../drag/registry';
import { pxToUs, usToPx } from '../scale';

/** A press that travels less than this (CSS px) is a click: it picks the key, not moves it. */
const DRAG_SLOP = 3;

interface KeyDiamondsProps {
  /** The layer's id: a move of its block moves the diamonds too. */
  id: string;
  startUs: number;
  durationUs: number;
  motion: Motion | undefined;
  pps: number;
  left: number;
  width: number;
  top: number;
  height: number;
  locked: boolean;
}

/**
 * A layer's motion keys on its block: a small diamond at each time any property has a key. A click selects the layer
 * and puts the playhead on the key; a drag moves every key at that time within the layer (Option drag copies them
 * there); Delete takes them away. Each is one undo step. Drawn beside the block, as its follower, so a move of the
 * block takes them along.
 */
export const KeyDiamonds = memo(function KeyDiamonds({
  id,
  startUs,
  durationUs,
  motion,
  pps,
  left,
  width,
  top,
  height,
  locked,
}: KeyDiamondsProps) {
  const ref = useCallback((el: HTMLDivElement | null) => registerFollower(id, el), [id]);
  const picked = useKeyPick((s) => s.picked);
  const press = useRef<{ fromUs: number; x: number; pointerId: number; moved: boolean } | null>(null);
  const times = motionKeyTimes(motion);
  if (times.length === 0) return null;
  const at = (us: number) => usToPx(us, pps);
  const pick = (us: number) => {
    useSelection.getState().select(id);
    usePlayback.getState().seek(startUs + us);
    useKeyPick.getState().pick({ kind: 'motion', itemId: id, tUs: us });
  };
  /** Where the press has dragged its key to, within the layer. */
  const dragged = (p: { fromUs: number; x: number }, clientX: number) => {
    return Math.min(durationUs, Math.max(0, Math.round(p.fromUs + pxToUs(clientX - p.x, pps))));
  };
  const onPointerDown = (e: PointerEvent<HTMLButtonElement>, us: number) => {
    if (e.button !== 0) return;
    // The block's own drag never starts from a diamond.
    e.stopPropagation();
    e.preventDefault();
    e.currentTarget.focus({ preventScroll: true });
    if (locked) return pick(us);
    e.currentTarget.setPointerCapture?.(e.pointerId);
    press.current = { fromUs: us, x: e.clientX, pointerId: e.pointerId, moved: false };
  };
  const onPointerMove = (e: PointerEvent<HTMLButtonElement>) => {
    const p = press.current;
    if (!p || e.pointerId !== p.pointerId) return;
    if (!p.moved && Math.abs(e.clientX - p.x) < DRAG_SLOP) return;
    p.moved = true;
    // Drawn straight onto the diamond: no render per move.
    e.currentTarget.style.left = `${at(dragged(p, e.clientX))}px`;
  };
  const onPointerUp = (e: PointerEvent<HTMLButtonElement>) => {
    const p = press.current;
    if (!p || e.pointerId !== p.pointerId) return;
    press.current = null;
    if (!p.moved && Math.abs(e.clientX - p.x) < DRAG_SLOP) return pick(p.fromUs);
    const toUs = dragged(p, e.clientX);
    e.currentTarget.style.left = `${at(p.fromUs)}px`;
    const item = itemNow(id);
    if (item && toUs !== p.fromUs) void moveKeysAt(item, p.fromUs, toUs, e.altKey);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, us: number) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      e.stopPropagation();
      return pick(us);
    }
    if (e.key !== 'Delete' && e.key !== 'Backspace') return;
    // The key, not its layer; once gone, Delete acts on the selection again.
    e.preventDefault();
    e.stopPropagation();
    useKeyPick.getState().clear();
    const item = itemNow(id);
    if (item && !locked) void deleteKeysAt(item, us);
  };
  return (
    <div ref={ref} className="tl-keys" style={{ left, width, top, height }}>
      {times.map((us) => (
        <button
          key={us}
          type="button"
          // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
          tabIndex={0}
          className="tl-key"
          data-picked={isPicked(picked, id, us) || undefined}
          style={{ left: at(us) }}
          aria-label={t({
            id: 'timeline.key.name',
            message: `Motion key at ${formatPrecise(startUs + us)}`,
            comment: 'A diamond on a layer’s block; {0} is a time on the timeline',
          })}
          title={t({
            id: 'timeline.key.tip',
            message: 'Drag to move. Option drag to copy. Delete removes it.',
            comment: 'Tooltip of a motion key’s diamond on the timeline',
          })}
          onPointerDown={(e) => onPointerDown(e, us)}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={(e) => {
            press.current = null;
            e.currentTarget.style.left = `${at(us)}px`;
          }}
          onClick={(e) => e.stopPropagation()}
          onKeyDown={(e) => onKeyDown(e, us)}
        />
      ))}
    </div>
  );
});
