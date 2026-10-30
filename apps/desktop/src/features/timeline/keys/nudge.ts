import { t } from '@lingui/core/macro';
import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import { formatTime } from '../../../lib/time';
import { findItem } from '../../../lib/timeline';
import { keyBurst } from '../../../state/keyBurst';
import { toast } from '../../../state/toasts';
import { useUi } from '../../../state/ui';
import { announce } from '../announce';
import { dragging } from '../drag/controller';
import { shiftBlock } from '../drag/registry';
import { mainIndex } from '../model/index';
import { attachedTo } from '../model/moveOps';
import { refitNote, reorderRefit } from '../model/refit';
import { reorderShifts } from '../model/reorder';
import { usToPx } from '../scale';
import { nudgeOps, nudgeReach } from './keyOps';
import { blockLabel, editable } from './timelineActions';
import type { LabelId } from '../../../lib/labels';

/** The blocks the settling nudge has drawn moved. */
const nudged = new Set<string>();

/**
 * Draws a settling nudge's `ops` at once (its commit lands after the burst): the layer, or the clip, the clips it
 * passes and their attached layers, move by transforms as a drag draws them; ones it no longer moves go back.
 */
function draw(project: Project, ops: Op[]) {
  const pps = useUi.getState().pxPerSecond;
  const now = new Map<string, number>();
  const op = ops[0];
  if (op?.type === 'moveItem') {
    const item = findItem(project, op.id)?.item;
    if (item) now.set(op.id, usToPx(op.startUs - item.startUs, pps));
  } else if (op?.type === 'moveClip') {
    const main = project.timeline.main;
    const index = mainIndex(main);
    const from = main.findIndex((c) => c.id === op.id);
    const to = op.index;
    const length = index.ends[from] - index.starts[from];
    const moved = reorderShifts(index, from, to);
    moved.set(from, (to > from ? index.ends[to] - length : index.starts[to]) - index.starts[from]);
    const attached = attachedTo(project.timeline.tracks);
    for (const [j, us] of moved)
      for (const id of [main[j].id, ...(attached.get(main[j].id) ?? [])]) now.set(id, usToPx(us, pps));
  }
  for (const id of nudged) if (!now.has(id)) shiftBlock(id, 0);
  nudged.clear();
  for (const [id, px] of now) {
    shiftBlock(id, px);
    nudged.add(id);
  }
}

/** Why block `id` (`found`, for a layer) can't move `steps` further: it's already first, last or at the start. */
function atLimit(found: ReturnType<typeof findItem>, steps: number): string {
  if (found)
    return found.item.content.type === 'text'
      ? t({ id: 'timeline.nudge.titleAtStart', message: 'The title is already at the start.' })
      : t({ id: 'timeline.nudge.layerAtStart', message: 'The layer is already at the start.' });
  return steps < 0
    ? t({ id: 'timeline.nudge.clipFirst', message: 'The clip is already first.' })
    : t({ id: 'timeline.nudge.clipLast', message: 'The clip is already last.' });
}

function press(id: string, steps: number, key: string, quiet: boolean) {
  // A press that waited for a commit and finds a drag under way does nothing: the drag owns the block (`editable`).
  const project = editable();
  if (!project) return;
  const found = findItem(project, id);
  // It says why it can't (as S does), once per held key (`quiet`: a repeat).
  if (found?.track.locked) {
    if (!quiet) toast.info(t({ id: 'timeline.trackLocked', message: 'This track is locked.' }));
    return;
  }
  const fps = project.settings.fps;
  const before = keyBurst.pending(key)?.total ?? 0;
  // Only what it can do adds up: presses past 0:00 or the end leave nothing to take back.
  const total = nudgeReach(project, id, before + steps, fps);
  if (total === before) {
    if (!quiet) toast.info(atLimit(found, steps));
    return;
  }
  const ops = nudgeOps(project, id, total, fps);
  const op = ops[0];
  const label = blockLabel(project, id);
  let said: string;
  let undo: LabelId;
  if (found) {
    const time = formatTime(op?.type === 'moveItem' ? op.startUs : 0);
    said = t({
      id: 'timeline.move.layer.said',
      message: `Moved ${label} to ${time}`,
      comment: 'Screen-reader note; {time} is a time in the video, e.g. 1:05',
    });
    undo = 'timeline.move.layer.undo';
  } else {
    const from = project.timeline.main.findIndex((c) => c.id === id);
    const to = op?.type === 'moveClip' ? op.index : from;
    const position = to + 1;
    // The model refits transitions to their new neighbours: the move says when it loses or shortens one.
    const note = refitNote(reorderRefit(project, from, to));
    said = note
      ? t({
          id: 'timeline.move.clip.saidNote',
          message: `Moved ${label} to position ${position}. ${note}`,
          comment:
            'Screen-reader note; {note} is a sentence on what it does to transitions, e.g. “A transition is removed.”',
        })
      : t({ id: 'timeline.move.clip.said', message: `Moved ${label} to position ${position}` });
    undo = 'timeline.move.clip.undo';
  }
  draw(project, ops);
  keyBurst.push(undo, ops, key, {
    total,
    done: (ok) => {
      // The new revision is already drawn (its render ran first): the blocks are where they were moved to.
      for (const shifted of nudged) shiftBlock(shifted, 0);
      nudged.clear();
      if (ok && ops.length > 0) announce(said);
    },
  });
}

/**
 * The , and . keys: block `id` earlier (negative `steps`) or later; `steps` frames for a layer, places for a clip.
 * The presses of a held key add up into one undo step (`keyBurst`), each building on what the burst has so far; a
 * press for another block, or during another commit (a drop's), first lets that go in. `quiet`: a held key's repeat,
 * which doesn't say again why it can't move.
 */
export function nudge(id: string, steps: number, quiet = false) {
  // A drag owns the block (and its transforms) until it lets go.
  if (dragging()) return;
  const key = `nudge:${id}`;
  if (keyBurst.pending(key)) press(id, steps, key, quiet);
  else keyBurst.after(() => press(id, steps, key, quiet), key);
}
