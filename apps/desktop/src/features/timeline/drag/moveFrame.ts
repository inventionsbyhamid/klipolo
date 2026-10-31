import { t } from '@lingui/core/macro';
import type { Project } from '../../../bindings/Project';
import type { TrackKind } from '../../../bindings/TrackKind';
import { formatPrecise, formatTime, snapToFrame } from '../../../lib/time';
import { findItem } from '../../../lib/timeline';
import { useUi } from '../../../state/ui';
import { mainIndex } from '../model/index';
import { attachedTo, moveClipOps, moveItemOps } from '../model/moveOps';
import { refitNote, reorderRefit } from '../model/refit';
import { insertionIndex, reorderShifts } from '../model/reorder';
import { snapEdges, snapTargets } from '../model/snap';
import { visibleRange } from '../model/visible';
import { pxToUs, usToPx } from '../scale';
import { SNAP_PX, type Gesture, type Session } from './session';
import { shiftBlock } from './registry';

const NONE: readonly string[] = [];

/**
 * One frame of a move: where the dragged block lands for the pointer `dxPx` (`dtUs`) from where it was pressed, drawn
 * through transforms (ids it moved go in `now`), and the ops that put it there (`s.ops`, `s.label`, `s.said`).
 * The ops come from the pure builders in `model/` that chat's tools use too. False: the block is gone.
 *
 * A clip lands after every clip whose middle the pointer passed; the clips in between slide aside, and every moved
 * clip's attached layers move with it.
 */
export function frameClip(
  s: Session,
  g: Extract<Gesture, { kind: 'clip' }>,
  project: Project,
  dxPx: number,
  dtUs: number,
  now: Set<string>,
): boolean {
  const main = project.timeline.main;
  if (s.fromMain !== main) {
    s.fromMain = main;
    s.from = main.findIndex((c) => c.id === g.clipId);
  }
  const from = s.from;
  if (from < 0) return false;
  const index = mainIndex(main);
  const to = insertionIndex(index, from, index.starts[from] + s.grabOffsetUs + dtUs);
  // The lanes mount the view and up to 1.5 views each side; a clip from up to its length further slides into that.
  const visUs = pxToUs(s.view.scroller.scrollLeft, s.pps);
  const reach = 2 * s.viewUs + (index.ends[from] - index.starts[from]);
  const near = visibleRange(index, visUs - reach, visUs + s.viewUs + reach);
  const attached = attachedTo(project.timeline.tracks);
  // A clip and the layers attached to it move together.
  const move = (id: string, px: number) => {
    shiftBlock(id, px);
    now.add(id);
    for (const layer of attached.get(id) ?? NONE) {
      shiftBlock(layer, px);
      now.add(layer);
    }
  };
  move(g.clipId, dxPx);
  for (const [j, us] of reorderShifts(index, from, to, near)) move(main[j].id, usToPx(us, s.pps));
  s.ops = moveClipOps(g.clipId, from, to);
  s.label = 'timeline.move.clip.undo';
  // The model refits transitions to their new neighbours: the drop says when it loses or shortens one.
  if (s.refit?.main !== main || s.refit.to !== to)
    s.refit = { main, to, text: refitNote(reorderRefit(project, from, to)) };
  const note = s.refit.text;
  const label = g.label;
  const position = to + 1;
  const count = main.length;
  s.said = note
    ? t({
        id: 'timeline.move.clip.saidNote',
        message: `Moved ${label} to position ${position}. ${note}`,
        comment:
          'Screen-reader note; {note} is a sentence on what it does to transitions, e.g. “A transition is removed.”',
      })
    : t({ id: 'timeline.move.clip.said', message: `Moved ${label} to position ${position}` });
  s.view.showTip(
    note
      ? t({
          id: 'timeline.move.clip.tipNote',
          message: `Position ${position} of ${count} · ${note}`,
          comment: 'Tip while dragging a clip; {note} is a sentence on what it does to transitions',
        })
      : t({ id: 'timeline.move.clip.tip', message: `Position ${position} of ${count}` }),
    s.last.x,
  );
  return true;
}

/**
 * A layer moves on the frame grid, never before 0, and its edges hold to the snap targets within 8 px unless snapping
 * is off or ⌘ is held; in expanded mode it moves to the unlocked track row of its kind under the pointer.
 */
export function frameItem(
  s: Session,
  g: Extract<Gesture, { kind: 'item' }>,
  project: Project,
  dtUs: number,
  now: Set<string>,
): boolean {
  const found = findItem(project, g.itemId);
  if (!found) return false;
  const { item, track } = found;
  // Layout reads first, before this frame writes any style. A queued drop uses the row found at its release.
  const row = s.dropRow !== undefined ? s.dropRow : s.view.rowAt(s.last.y, g.trackKind);
  let toTrackId = track.id;
  let dy = 0;
  let newTrack: { id: string; kind: TrackKind; index: number } | undefined;
  if (row && 'newTrack' in row) {
    // Appended, as `rowAt` found it, to the project the ops build on (a queued drop's may have gained a track since).
    newTrack = { id: s.newTrackId, kind: g.trackKind, index: project.timeline.tracks.length };
    dy = row.dy;
  } else if (row) {
    // A locked track takes no layers; the layer stays on its own row.
    const target = project.timeline.tracks.find((t) => t.id === row.trackId);
    if (target && !target.locked && target.kind === track.kind) {
      toTrackId = target.id;
      dy = row.dy;
    }
  }
  let start = Math.max(0, snapToFrame(item.startUs + dtUs, s.fps));
  if (useUi.getState().snapping && !s.last.meta) {
    // Never back onto where the layer started: a playhead parked there by an earlier click, a cut or another
    // layer's edge it already sits on would swallow every nudge under the snap distance.
    const from = [item.startUs, item.startUs + item.durationUs];
    s.targets ??= snapTargets(project, s.playheadUs, new Set([item.id])).filter((t) => !from.includes(t));
    const hit = snapEdges([start, start + item.durationUs], s.targets, pxToUs(SNAP_PX, s.pps));
    if (hit) start = Math.max(0, start + hit.deltaUs);
    s.view.showGuide(hit ? hit.atUs : null);
  } else s.view.showGuide(null);
  shiftBlock(item.id, usToPx(start - item.startUs, s.pps), dy);
  now.add(item.id);
  s.ops = moveItemOps(item, track.id, toTrackId, start, newTrack);
  s.label = newTrack ? 'timeline.move.layerNewTrack.undo' : 'timeline.move.layer.undo';
  const label = g.label;
  const time = formatTime(start);
  s.said = newTrack
    ? t({
        id: 'timeline.move.layerNewTrack.said',
        message: `Moved ${label} to a new track at ${time}`,
        comment: 'Screen-reader note; {time} is a time in the video, e.g. 1:05',
      })
    : t({
        id: 'timeline.move.layer.said',
        message: `Moved ${label} to ${time}`,
        comment: 'Screen-reader note; {time} is a time in the video, e.g. 1:05',
      });
  s.view.showTip(formatPrecise(start), s.last.x);
  return true;
}
