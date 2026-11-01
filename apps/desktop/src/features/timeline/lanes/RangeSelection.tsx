import { useLingui } from '@lingui/react/macro';
import type { PointerEvent as ReactPointerEvent } from 'react';
import { snapToFrame } from '../../../lib/time';
import { videoDurationUs } from '../../../lib/timeline';
import { formatTime } from '../../../player/time';
import { useProject } from '../../../state/project';
import { useRange, type TimeRange } from '../../../state/range';
import { useSelection } from '../../../state/selection';
import { blocksTouching, type ScreenBox } from '../drag/registry';
import { useUi } from '../../../state/ui';
import { RULER_PX } from '../layout';
import { pxToUs, usToPx } from '../scale';

/** A press that moves less than this (px) is a click (a seek), not a range. */
const DRAG_PX = 3;

/** The selection's length as its tag shows it: "5.6 s" under a minute, else "1:05.2". */
export function rangeLength(range: TimeRange): string {
  const us = range.endUs - range.startUs;
  return us < 60_000_000 ? `${(Math.floor(us / 100_000) / 10).toFixed(1)} s` : formatTime(us);
}

/** The time at `clientX` over the lanes whose left edge (the video's start) is at `laneLeft`, on the frame grid. */
function timeAt(clientX: number, laneLeft: number): number {
  const project = useProject.getState().snapshot?.project;
  if (!project) return 0;
  const us = pxToUs(Math.max(0, clientX - laneLeft), useUi.getState().pxPerSecond);
  return Math.min(snapToFrame(us, project.settings.fps), videoDurationUs(project.timeline));
}

/** A drag just made or changed a range: the click that ends it doesn't also seek. */
let dragged = false;
export function rangeDragJustEnded(): boolean {
  const was = dragged;
  dragged = false;
  return was;
}

/** The box a lane drag draws (a fixed element over the page, made once and kept). */
let boxEl: HTMLElement | null = null;
function showBox(box: ScreenBox | null) {
  if (box === null) {
    if (boxEl) boxEl.style.display = 'none';
    return;
  }
  if (!boxEl || !boxEl.isConnected) {
    boxEl = document.createElement('div');
    boxEl.className = 'tl-box';
    boxEl.setAttribute('aria-hidden', 'true');
    document.body.append(boxEl);
  }
  Object.assign(boxEl.style, {
    display: 'block',
    left: `${box.left}px`,
    top: `${box.top}px`,
    width: `${box.right - box.left}px`,
    height: `${box.bottom - box.top}px`,
  });
}

/**
 * A press on empty lane space or the ruler, at `e` over `lane` (an element whose left edge is the video's start): once
 * the pointer has moved 3 px it selects the time from the press to the pointer, and the block
 * selection goes (one selection at a time). A press that doesn't move leaves things as they are: its click seeks.
 *
 * On the lanes (`box`), the drag draws a box from the press to the pointer: while it touches clips or layers it
 * selects them (and no time); touching none, it selects the time as before.
 */
export function startRangeDrag(e: ReactPointerEvent<HTMLElement>, lane: HTMLElement, box = false) {
  if (e.button !== 0) return;
  const el = e.currentTarget;
  const pointerId = e.pointerId;
  const fromX = e.clientX;
  const fromY = e.clientY;
  const laneLeft = lane.getBoundingClientRect().left;
  const fromUs = timeAt(fromX, laneLeft);
  let moving = false;
  dragged = false;
  const move = (ev: PointerEvent) => {
    if (ev.pointerId !== pointerId) return;
    const far = Math.max(Math.abs(ev.clientX - fromX), box ? Math.abs(ev.clientY - fromY) : 0);
    if (!moving && far < DRAG_PX) return;
    if (!moving) {
      // Captured once it is a drag, never at the press: a press that doesn't move is a click, which seeks.
      moving = true;
      capture(el, pointerId);
      useSelection.getState().select(null);
    }
    if (box) {
      const drawn = {
        left: Math.min(fromX, ev.clientX),
        right: Math.max(fromX, ev.clientX),
        top: Math.min(fromY, ev.clientY),
        bottom: Math.max(fromY, ev.clientY),
      };
      showBox(drawn);
      const touched = blocksTouching(drawn);
      if (touched.length > 0) {
        useRange.getState().clear();
        useSelection.getState().selectMany(touched);
        return;
      }
      if (useSelection.getState().selectedId !== null) useSelection.getState().select(null);
    }
    useRange.getState().select(fromUs, timeAt(ev.clientX, laneLeft));
  };
  const end = (ev: PointerEvent) => {
    if (ev.pointerId !== pointerId) return;
    stop();
    if (moving) dragged = true;
  };
  followPointer(move, end);
}

/**
 * Hears the pointer's moves and its release or cancel on the window, wherever they happen (a release over another
 * panel too), until `end` runs; returns the stop. As `drag/controller` does: a gesture never outlives its release.
 */
let stop: () => void = () => undefined;
function followPointer(move: (e: PointerEvent) => void, end: (e: PointerEvent) => void) {
  stop();
  window.addEventListener('pointermove', move);
  window.addEventListener('pointerup', end);
  window.addEventListener('pointercancel', end);
  stop = () => {
    showBox(null);
    window.removeEventListener('pointermove', move);
    window.removeEventListener('pointerup', end);
    window.removeEventListener('pointercancel', end);
    stop = () => undefined;
  };
}

/** Ends a range or grip drag under way: the timeline is going (unmounted mid-drag), and nothing follows the pointer. */
export function cancelRangeDrag() {
  stop();
}

/** Takes the pointer's events while `el` is still in the page: one gone (the timeline unmounted) can't hold them. */
function capture(el: HTMLElement, pointerId: number) {
  if (!el.isConnected) return;
  try {
    el.setPointerCapture?.(pointerId);
  } catch {
    // The pointer is no longer active (released elsewhere a moment ago): the window still hears it.
  }
}

/** Drag one of the selection's grips: that edge follows the pointer, the other stays. */
function gripDrag(e: ReactPointerEvent<HTMLElement>, edge: 'start' | 'end') {
  if (e.button !== 0) return;
  e.stopPropagation();
  const range = useRange.getState().range;
  const lane = e.currentTarget.parentElement?.parentElement;
  if (!range || !lane) return;
  const pointerId = e.pointerId;
  capture(e.currentTarget, pointerId);
  const laneLeft = lane.getBoundingClientRect().left;
  const keep = edge === 'start' ? range.endUs : range.startUs;
  followPointer(
    (ev) => {
      if (ev.pointerId === pointerId) useRange.getState().select(keep, timeAt(ev.clientX, laneLeft));
    },
    (ev) => {
      if (ev.pointerId === pointerId) stop();
    },
  );
}

/**
 * The selected time range over every lane, from the ruler's bottom: a paper wash with paper edges, two
 * grips at mid height to change it, and its length in a tag at the top. It sits in the lanes' overlay (x 0 is the
 * video's start); only the grips take the pointer.
 */
export function RangeSelection({ pps }: { pps: number }) {
  const { t } = useLingui();
  const range = useRange((s) => s.range);
  if (!range) return null;
  const left = usToPx(range.startUs, pps);
  const width = Math.max(2, usToPx(range.endUs, pps) - left);
  const from = formatTime(range.startUs);
  const to = formatTime(range.endUs);
  return (
    <div
      data-range
      role="note"
      aria-label={t({
        id: 'timeline.range.name',
        message: `Selection ${from} to ${to}`,
        comment: 'The time range selected on the timeline; {from} and {to} are times like 4:26.2',
      })}
      className="tl-range"
      style={{ left, width, top: RULER_PX + 2 }}
    >
      <span className="tl-range-len">{rangeLength(range)}</span>
      <span aria-hidden data-grip="start" className="tl-grip is-start" onPointerDown={(e) => gripDrag(e, 'start')} />
      <span aria-hidden data-grip="end" className="tl-grip is-end" onPointerDown={(e) => gripDrag(e, 'end')} />
    </div>
  );
}
