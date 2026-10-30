import { useLingui } from '@lingui/react/macro';
import type { PointerEvent as ReactPointerEvent } from 'react';
import { snapToFrame } from '../../../lib/time';
import { videoDurationUs } from '../../../lib/timeline';
import { formatTime } from '../../../player/time';
import { useProject } from '../../../state/project';
import { useRange, type TimeRange } from '../../../state/range';
import { useSelection } from '../../../state/selection';
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

/**
 * A press on empty lane space or the ruler, at `e` over `lane` (an element whose left edge is the video's start): once
 * the pointer has moved 3 px it selects the time from the press to the pointer, and the block
 * selection goes (one selection at a time). A press that doesn't move leaves things as they are: its click seeks.
 */
export function startRangeDrag(e: ReactPointerEvent<HTMLElement>, lane: HTMLElement) {
  if (e.button !== 0) return;
  const el = e.currentTarget;
  const pointerId = e.pointerId;
  const fromX = e.clientX;
  const laneLeft = lane.getBoundingClientRect().left;
  const fromUs = timeAt(fromX, laneLeft);
  let moving = false;
  dragged = false;
  const move = (ev: PointerEvent) => {
    if (ev.pointerId !== pointerId) return;
    if (!moving && Math.abs(ev.clientX - fromX) < DRAG_PX) return;
    if (!moving) {
      moving = true;
      el.setPointerCapture?.(pointerId);
      useSelection.getState().select(null);
    }
    useRange.getState().select(fromUs, timeAt(ev.clientX, laneLeft));
  };
  const end = (ev: PointerEvent) => {
    if (ev.pointerId !== pointerId) return;
    el.removeEventListener('pointermove', move);
    el.removeEventListener('pointerup', end);
    el.removeEventListener('pointercancel', end);
    if (moving) dragged = true;
  };
  el.addEventListener('pointermove', move);
  el.addEventListener('pointerup', end);
  el.addEventListener('pointercancel', end);
}

/** Drag one of the selection's grips: that edge follows the pointer, the other stays. */
function gripDrag(e: ReactPointerEvent<HTMLElement>, edge: 'start' | 'end') {
  if (e.button !== 0) return;
  e.stopPropagation();
  const range = useRange.getState().range;
  const lane = e.currentTarget.parentElement?.parentElement;
  if (!range || !lane) return;
  const el = e.currentTarget;
  el.setPointerCapture?.(e.pointerId);
  const laneLeft = lane.getBoundingClientRect().left;
  const keep = edge === 'start' ? range.endUs : range.startUs;
  const move = (ev: PointerEvent) => useRange.getState().select(keep, timeAt(ev.clientX, laneLeft));
  const end = () => {
    el.removeEventListener('pointermove', move);
    el.removeEventListener('pointerup', end);
    el.removeEventListener('pointercancel', end);
  };
  el.addEventListener('pointermove', move);
  el.addEventListener('pointerup', end);
  el.addEventListener('pointercancel', end);
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
