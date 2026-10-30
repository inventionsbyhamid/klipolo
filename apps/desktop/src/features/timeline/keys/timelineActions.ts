import { t } from '@lingui/core/macro';
import type { Edge } from '../../../bindings/Edge';
import type { Op } from '../../../bindings/Op';
import type { Project } from '../../../bindings/Project';
import { labelParts, type Label, type LabelId } from '../../../lib/labels';
import { formatPrecise, formatTime, snapToFrame } from '../../../lib/time';
import {
  clipDurationUs,
  findClip,
  findItem,
  itemEndUs,
  itemLabel,
  sourceLabel,
  videoDurationUs,
} from '../../../lib/timeline';
import { beginCommit } from '../../../state/commitQueue';
import { keyBurst } from '../../../state/keyBurst';
import { usePlayback } from '../../../state/playback';
import { useProject } from '../../../state/project';
import { useRange, type TimeRange } from '../../../state/range';
import { useSelection } from '../../../state/selection';
import { toast } from '../../../state/toasts';
import { useTimelineFocus } from '../../../state/timelineFocus';
import { useUi } from '../../../state/ui';
import { announce } from '../announce';
import { dragging } from '../drag/controller';
import { editPoints, nextEdit, previousEdit } from '../model/editPoints';
import { clipIndexAt, mainIndex } from '../model/index';
import { deleteRangeOps } from '../model/rangeOps';
import { refitNote, trimRefit } from '../model/refit';
import { deleteOps, trimToPlayheadOps } from './keyOps';
import { focusIdOf, focusSelection, laneOrder, stepFrom, type LaneKey } from './laneOrder';
import { isMarker } from './markers';

/*
 * What the timeline's commands do (`useTimelineCommands`; the clipboard's are in `clipboardActions`, nudges in
 * `nudge`, a layer's track and clip in `layerActions`, tracks in `trackActions`). Each reads the project when it runs
 * (after a nudge still settling has gone in: `keyBurst.after`), builds its ops with the pure builders in `keyOps`,
 * `clipboardOps` and `model/rangeOps`, commits them as one undo step through `useProject.apply` and says what it did
 * in the timeline's live region.
 */

/**
 * The project, when it can be edited: not a newer Klipolo build's (it opens read-only), and not while a drag owns the
 * timeline (an edit key pressed mid-drag, or one that waited for a commit and finds a drag lifted, does nothing).
 */
export const editable = () => {
  const snapshot = useProject.getState().snapshot;
  return snapshot && !snapshot.readOnly && !dragging() ? snapshot.project : null;
};

/** A clip's or layer's name: its file, its text. */
export function blockLabel(project: Project, id: string): string {
  const found = findItem(project, id);
  if (found) return itemLabel(project, found.item);
  const clip = findClip(project, id);
  return clip ? sourceLabel(project, clip.source) : '';
}

/** Whether a timeline block has focus (the keyboard is working in the timeline). */
export function blockFocused(): boolean {
  const el = document.activeElement;
  return el instanceof HTMLElement && el.dataset.blockId !== undefined;
}

/**
 * Keyboard focus moves to block `id` (selecting it): the view mounts it, scrolls to it if it's out of view and
 * focuses it. The arrow keys and the Select previous / next / above / below commands go through here.
 */
export function moveFocusTo(project: Project, id: string) {
  useSelection.getState().select(focusSelection(project, id));
  useTimelineFocus.getState().focus(id, true);
}

/** Where `key` takes focus from block `id` in the lanes as they draw now; null: nowhere. */
export function focusStep(project: Project, id: string, key: LaneKey): string | null {
  return stepFrom(laneOrder(project, useUi.getState().timelineExpanded), id, key);
}

/**
 * Where `key` takes the selection from selection `selectedId` (Select next, previous…): the next block that selects
 * something, so a bare cut between two clips is passed over (a transition at a cut is selected). Null: nowhere.
 */
export function selectionStep(project: Project, selectedId: string, key: LaneKey): string | null {
  let id = focusIdOf(selectedId);
  // Cuts and clips alternate on the Video lane, so this passes over one bare cut at most.
  for (let n = 0; n < 2; n++) {
    const next = focusStep(project, id, key);
    if (next === null || focusSelection(project, next) !== null) return next;
    id = next;
  }
  return null;
}

/**
 * A block an edit made (pasted, duplicated) is selected, and takes focus when a block had it, or a "+N" marker standing
 * for one: focus follows the selection, so the two never name different blocks.
 */
export function selectNew(id: string | null) {
  if (id === null) return;
  useSelection.getState().select(id);
  if (blockFocused() || isMarker(document.activeElement)) useTimelineFocus.getState().focus(id, true);
}

/** "The playhead isn't inside the selected clip." (or title, or layer): why S, Q or W can't act on block `id`. */
export function notInside(project: Project, id: string): string {
  const item = findItem(project, id)?.item;
  if (!item)
    return t({ id: 'timeline.split.notInsideClip', message: "The playhead isn't inside the selected clip." });
  return item.content.type === 'text'
    ? t({ id: 'timeline.split.notInsideTitle', message: "The playhead isn't inside the selected title." })
    : t({ id: 'timeline.split.notInsideLayer', message: "The playhead isn't inside the selected layer." });
}

/**
 * One undo step; `said` is announced once it has gone in, then `then` runs. It holds the timeline's commit queue until
 * it lands, so a drag or another edit builds on it. Call it only at an edit's turn, inside `keyBurst.after` (every
 * caller does): it sends at once, so called elsewhere it wouldn't wait for a commit already on its way.
 */
export function commit(label: Label, ops: Op[], said: string, then?: () => void) {
  if (ops.length === 0) return;
  const end = beginCommit();
  const [id, params] = labelParts(label);
  // eslint-disable-next-line no-restricted-syntax -- sent at its turn (`keyBurst.after`), holding the commit queue
  void useProject
    .getState()
    .apply(id, ops, params)
    .then((ok) => {
      if (!ok) return;
      announce(said);
      then?.();
    })
    .finally(end);
}

/**
 * S: the selected layer (`selectedId`) at the playhead, else the clip there. With a layer selected it acts on that
 * layer alone, and says why it can't rather than splitting something else. From a clip's menu (`block`), it splits
 * that clip alone, the same way.
 */
export function split(selectedId: string | null, block?: string) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const atUs = snapToFrame(usePlayback.getState().playheadUs, project.settings.fps);
    const time = formatTime(atUs);
    const said = (label: string) =>
      t({ id: 'timeline.key.split', message: `Split ${label} at ${time}`, comment: '{time} is e.g. 1:05' });
    const found = selectedId ? findItem(project, selectedId) : null;
    if (!found) {
      const index = mainIndex(project.timeline.main);
      const i = block === undefined ? -1 : project.timeline.main.findIndex((c) => c.id === block);
      if (i >= 0 && (atUs <= index.starts[i] || atUs >= index.ends[i]))
        return toast.info(notInside(project, block!));
      const under = project.timeline.main[clipIndexAt(index, atUs)];
      const label = under ? sourceLabel(project, under.source) : '';
      return commit('undo.split.clip', [{ type: 'split', atUs }], said(label));
    }
    const { item, track } = found;
    if (track.locked) return toast.info(t({ id: 'timeline.trackLocked', message: 'This track is locked.' }));
    if (atUs <= item.startUs || atUs >= itemEndUs(item)) return toast.info(notInside(project, item.id));
    commit('undo.split.layer', [{ type: 'splitItem', id: item.id, atUs }], said(itemLabel(project, item)));
  });
}

/**
 * Q / W: trims `edge` of block `id` to the playhead. Like S, it says why it can't (a locked track, the playhead outside
 * the block), unless `quiet` (a held key's repeat).
 */
export function trimToPlayhead(id: string, edge: Edge, quiet = false) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const playheadUs = usePlayback.getState().playheadUs;
    const ops = trimToPlayheadOps(project, id, playheadUs, edge, project.settings.fps);
    if (ops.length === 0 && !quiet) {
      if (findItem(project, id)?.track.locked)
        return toast.info(t({ id: 'timeline.trackLocked', message: 'This track is locked.' }));
      return toast.info(notInside(project, id));
    }
    const op = ops[0];
    const label = blockLabel(project, id);
    if (op?.type === 'trimItem') {
      const item = findItem(project, id)!.item;
      const length = formatPrecise(edge === 'start' ? itemEndUs(item) - op.atUs : op.atUs - item.startUs);
      const said = t({ id: 'timeline.trim.said', message: `Trimmed ${label} to ${length}` });
      return commit('timeline.trim.layer.undo', ops, said);
    }
    if (op?.type !== 'trimClip') return;
    const main = project.timeline.main;
    const i = main.findIndex((c) => c.id === id);
    const trimmed = { ...main[i], [edge === 'start' ? 'sourceInUs' : 'sourceOutUs']: op.sourceUs };
    const length = formatPrecise(clipDurationUs(trimmed));
    // The model refits the transitions into and out of the clip: the trim says when it loses or shortens one.
    const note = refitNote(trimRefit(project, i, trimmed));
    const said = note
      ? t({
          id: 'timeline.trim.saidNote',
          message: `Trimmed ${label} to ${length}. ${note}`,
          comment:
            'Screen-reader note; {note} is a sentence on what it does to transitions, e.g. “A transition is removed.”',
        })
      : t({ id: 'timeline.trim.said', message: `Trimmed ${label} to ${length}` });
    commit('timeline.trim.clip.undo', ops, said);
  });
}

/** ⌫: block `id` out of the video; `undo` is the command's label. */
export function deleteBlock(id: string, undo: LabelId) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const label = blockLabel(project, id);
    commit(undo, deleteOps(project, id), t({ id: 'timeline.key.deleted', message: `Deleted ${label}` }));
  });
}

/**
 * Delete with a time range selected: its footage goes in one `deleteRanges` step (the gap closes, attached layers
 * follow), the range clears and the playhead waits where it began. A range past the clips says there is nothing to
 * take out there.
 */
export function deleteRange(range: TimeRange) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const ops = deleteRangeOps(project, range);
    if (ops.length === 0) {
      toast.info(t({ id: 'timeline.range.nothing', message: 'There is no video in the selection to delete.' }));
      return;
    }
    const length = formatPrecise(range.endUs - range.startUs);
    commit('undo.delete.range', ops, t({ id: 'timeline.range.deleted', message: `Deleted ${length} of video` }), () => {
      useRange.getState().clear();
      const now = useProject.getState().snapshot?.project;
      const end = now ? videoDurationUs(now.timeline) : 0;
      usePlayback.getState().seek(Math.min(range.startUs, end));
    });
  });
}

/** ↑ / ↓: the playhead to the previous (−1) or next (1) edit point, never past the end of the video. */
export function seekEdit(direction: 1 | -1) {
  const project = useProject.getState().snapshot?.project;
  if (!project) return;
  const durationUs = videoDurationUs(project.timeline);
  const { playheadUs, seek } = usePlayback.getState();
  const points = editPoints(project);
  const at =
    direction > 0 ? nextEdit(points, playheadUs) : previousEdit(points, Math.min(playheadUs, durationUs));
  if (at !== null) seek(Math.min(at, durationUs));
}
