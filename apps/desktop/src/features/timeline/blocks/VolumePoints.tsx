import { t } from '@lingui/core/macro';
import { memo, useCallback, useRef, type KeyboardEvent, type PointerEvent } from 'react';
import { movedKey } from '../../../lib/keys';
import { formatPrecise } from '../../../lib/time';
import { deleteVolumeKey, moveVolumeKey, type Leveled } from '../../../state/keyEdits';
import { isPicked, pickMoved, useKeyPick } from '../../../state/keyPick';
import { usePlayback } from '../../../state/playback';
import { registerFollower } from '../drag/registry';
import { useSpeech } from '../media/speech';
import { usToPx } from '../scale';
import { levelAt, levelPoints, levelY, pointY, previewLine, type LineLevel } from './VolumeLine';

/** A press that travels less than this (CSS px) is a click: it picks the point, not moves it. */
const DRAG_SLOP = 3;

interface VolumePointsProps {
  /** The block whose line they are on: a move of the block moves them, and a drag redraws its line. */
  id: string;
  startUs: number;
  durationUs: number;
  content: LineLevel;
  target: Leveled;
  pps: number;
  /** The line's box in the lane (px): the block's left, width and top, and the height its line is drawn in. */
  left: number;
  width: number;
  top: number;
  height: number;
}

interface Press {
  fromUs: number;
  v: number;
  x: number;
  y: number;
  pointerId: number;
  moved: boolean;
}

/**
 * A volume line's keys: a point at each, on the line, beside the block (as its follower), so each is a button of its
 * own that takes the focus and is named by its time and level. A press picks it (Delete then takes it away) and puts
 * the playhead there; a drag moves it in time and level, redrawing the line as it goes; Delete removes it. Each change
 * is one undo step.
 */
export const VolumePoints = memo(function VolumePoints({
  id,
  startUs,
  durationUs,
  content,
  target,
  pps,
  left,
  width,
  top,
  height,
}: VolumePointsProps) {
  const ref = useCallback((el: HTMLDivElement | null) => registerFollower(id, el), [id]);
  const speech = useSpeech((s) => s.ranges);
  const picked = useKeyPick((s) => s.picked);
  const press = useRef<Press | null>(null);
  const keys = content.volumeKeys ?? [];
  if (keys.length === 0) return null;
  const drawnWidth = Math.max(1, usToPx(durationUs, pps));
  const points = levelPoints(durationUs, startUs, content, speech);
  const xOf = (us: number) => (us / durationUs) * drawnWidth;
  /** The keys with the pressed point moved to where the pointer is, and where that put it. */
  const moved = (p: Press, clientX: number, clientY: number) => {
    const v = Math.round(levelAt(levelY(p.v) + (clientY - p.y) / Math.max(1, height)) * 100) / 100;
    const next = movedKey(keys, p.fromUs, p.fromUs + ((clientX - p.x) / drawnWidth) * durationUs, durationUs);
    const toUs = next.find((k) => !keys.includes(k))?.tUs ?? p.fromUs;
    return { keys: next.map((k) => (keys.includes(k) ? k : { ...k, v })), toUs, v };
  };
  const pick = (us: number) => {
    useKeyPick.getState().pick({ kind: 'volume', target, tUs: us });
    usePlayback.getState().seek(startUs + us);
  };
  const onPointerDown = (e: PointerEvent<HTMLButtonElement>, us: number, v: number) => {
    if (e.button !== 0) return;
    // The block's own drag never starts from a point.
    e.stopPropagation();
    e.preventDefault();
    e.currentTarget.focus({ preventScroll: true });
    e.currentTarget.setPointerCapture?.(e.pointerId);
    useKeyPick.getState().pick({ kind: 'volume', target, tUs: us });
    press.current = { fromUs: us, v, x: e.clientX, y: e.clientY, pointerId: e.pointerId, moved: false };
  };
  const onPointerMove = (e: PointerEvent<HTMLButtonElement>) => {
    const p = press.current;
    if (!p || e.pointerId !== p.pointerId) return;
    if (!p.moved && Math.hypot(e.clientX - p.x, e.clientY - p.y) < DRAG_SLOP) return;
    p.moved = true;
    const next = moved(p, e.clientX, e.clientY);
    // Drawn straight onto the point and the line: no render per move.
    e.currentTarget.style.left = `${xOf(next.toUs)}px`;
    e.currentTarget.style.top = `${levelY(next.v) * height}px`;
    previewLine(id, next.keys);
  };
  const settle = (el: HTMLElement, us: number) => {
    el.style.left = `${xOf(us)}px`;
    el.style.top = `${pointY(points, us, height)}px`;
  };
  const onPointerUp = (e: PointerEvent<HTMLButtonElement>) => {
    const p = press.current;
    if (!p || e.pointerId !== p.pointerId) return;
    press.current = null;
    const click = !p.moved && Math.hypot(e.clientX - p.x, e.clientY - p.y) < DRAG_SLOP;
    if (click) return pick(p.fromUs);
    const { toUs, v } = moved(p, e.clientX, e.clientY);
    settle(e.currentTarget, p.fromUs);
    const landed = moveVolumeKey(target, p.fromUs, toUs, v);
    // Delete straight after takes the point where it went, not the clip or sound. This point lets go of the focus (it
    // stays drawn here until the move lands), so Delete goes to the pick.
    e.currentTarget.blur();
    pickMoved({ kind: 'volume', target, tUs: toUs }, p.fromUs, landed);
    void landed.then(() => previewLine(id, null));
  };
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, us: number) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      e.stopPropagation();
      return pick(us);
    }
    if (e.key !== 'Delete' && e.key !== 'Backspace') return;
    // The key, not the clip or sound; once gone, Delete acts on the selection again.
    e.preventDefault();
    e.stopPropagation();
    useKeyPick.getState().clear();
    void deleteVolumeKey(target, us);
  };
  return (
    <div ref={ref} className="tl-volume-keys" style={{ left, width, top, height }}>
      {keys.map((k) => (
        <button
          key={k.tUs}
          type="button"
          // WebKit leaves plain buttons out of Tab order unless Full Keyboard Access is on: every control is reachable.
          tabIndex={0}
          className="tl-volume-key"
          style={{ left: xOf(k.tUs), top: pointY(points, k.tUs, height) }}
          data-picked={isPicked(picked, target.id, k.tUs) || undefined}
          aria-label={t({
            id: 'volume.key.name',
            message: `Volume key at ${formatPrecise(startUs + k.tUs)}, ${Math.round(k.v * 100)}%`,
            comment: 'A point on a volume line; {0} is a time on the timeline, {1} its level',
          })}
          title={t({
            id: 'volume.key.tip',
            message: 'Drag to move. Delete removes it.',
            comment: 'Tooltip of a volume key’s point on the timeline',
          })}
          onPointerDown={(e) => onPointerDown(e, k.tUs, k.v)}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerCancel={(e) => {
            press.current = null;
            settle(e.currentTarget, k.tUs);
            previewLine(id, null);
          }}
          onClick={(e) => e.stopPropagation()}
          onKeyDown={(e) => onKeyDown(e, k.tUs)}
        />
      ))}
    </div>
  );
});
