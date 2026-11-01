import { t } from '@lingui/core/macro';
import type { Project } from '../../../bindings/Project';
import type { LabelId } from '../../../lib/labels';
import { frameUs } from '../../../lib/time';
import { useClipboard, type GroupEntry } from '../../../state/clipboard';
import { keyBurst } from '../../../state/keyBurst';
import { usePlayback } from '../../../state/playback';
import { useProject } from '../../../state/project';
import { groupOf, selectionOf, useSelection } from '../../../state/selection';
import { toast } from '../../../state/toasts';
import { announce } from '../announce';
import {
  countOf,
  deletedCount,
  everyBlock,
  groupDeleteOps,
  groupDuplicateOps,
  groupEntries,
  groupMoveOps,
  groupMuteOps,
  groupPasteEffectsOps,
  groupPasteOps,
  groupVolumeOps,
  membersOf,
  rangeOnTrack,
  type Members,
  type MoveRefusal,
} from './groupOps';
import { commit, editable } from './timelineActions';

/*
 * A selected group's edits (`groupOps`), each one undo step labelled with how many blocks it touched ("Delete 3
 * items"), said in the timeline's live region once it has gone in.
 */

/** The group selected now, as the project has it (null: one block or none is selected). */
export function selectedGroup(project: Project): Members | null {
  const ids = groupOf(useSelection.getState());
  return ids ? membersOf(project, ids) : null;
}

/** `id` with `count`, as a label the history words ("Delete 3 items"). */
const counted = (id: LabelId, count: number) => ({ id, params: { count } });

/** "3 items": how many blocks a group holds, as its card's title and a screen reader say it. */
export function itemsWord(count: number): string {
  return t({ id: 'timeline.group.items', message: `${count} items`, comment: 'Several selected clips and layers' });
}

/** The sentence a refused group move says. */
export function moveRefusal(why: MoveRefusal): string {
  return why === 'locked'
    ? t({ id: 'timeline.group.lockedMove', message: "A layer in the selection is on a locked track, so they can't move." })
    : why === 'kind'
      ? t({ id: 'timeline.group.kindMove', message: 'Each layer needs a track of its own kind there.' })
      : t({ id: 'timeline.group.startMove', message: "The selection can't move before the start of the video." });
}

/** Runs `then` with the group as the project has it once a nudge still settling has gone in. */
function withGroup(then: (project: Project, m: Members) => void) {
  keyBurst.after(() => {
    const project = editable();
    const m = project ? selectedGroup(project) : null;
    if (project && m && countOf(m) > 0) then(project, m);
  });
}

/** ⌫ with a group: every block of it out in one step; the selection clears. */
export function deleteGroup() {
  withGroup((project, m) => {
    const count = deletedCount(m);
    if (count === 0) return toast.info(t({ id: 'timeline.trackLocked', message: 'This track is locked.' }));
    const said = t({ id: 'timeline.group.deleted', message: `Deleted ${count} items` });
    commit(counted('timeline.group.delete', count), groupDeleteOps(project, m), said, () =>
      useSelection.getState().select(null),
    );
  });
}

/** ⌘C with a group: its blocks onto the clipboard, each at its distance from the first (nothing is edited). */
export function copyGroup() {
  const project = useProject.getState().snapshot?.project;
  const m = project ? selectedGroup(project) : null;
  if (!project || !m || countOf(m) === 0) return;
  const entries = groupEntries(project, m);
  useClipboard.setState({ projectId: project.id, entry: entries[0].entry, group: entries, effects: null });
  announce(t({ id: 'timeline.group.copied', message: `Copied ${countOf(m)} items` }));
}

/** ⌘X with a group: copied, then deleted, as one step called Cut. */
export function cutGroup() {
  withGroup((project, m) => {
    const ops = groupDeleteOps(project, m);
    if (ops.length === 0) return;
    const entries = groupEntries(project, m);
    useClipboard.setState({ projectId: project.id, entry: entries[0].entry, group: entries, effects: null });
    const count = deletedCount(m);
    commit(
      counted('timeline.group.cut', count),
      ops,
      t({ id: 'timeline.group.cutSaid', message: `Cut ${count} items` }),
      () => useSelection.getState().select(null),
    );
  });
}

/** ⌘V of a copied group: at the playhead, its offsets kept; the copies are selected. */
export function pasteGroup(entries: GroupEntry[]) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const [ops, ids] = groupPasteOps(project, entries, usePlayback.getState().playheadUs);
    const said = t({ id: 'timeline.group.pasted', message: `Pasted ${ids.length} items` });
    commit(counted('timeline.group.paste', ids.length), ops, said, () => useSelection.getState().selectMany(ids));
  });
}

/** ⌘D with a group: copies right after it, which are then selected. */
export function duplicateGroup() {
  withGroup((project, m) => {
    const [ops, ids] = groupDuplicateOps(project, m);
    if (ops.length === 0) return toast.info(t({ id: 'timeline.trackLocked', message: 'This track is locked.' }));
    const said = t({ id: 'timeline.group.duplicated', message: `Duplicated ${ids.length} items` });
    commit(counted('timeline.group.duplicate', ids.length), ops, said, () =>
      useSelection.getState().selectMany(ids),
    );
  });
}

/** ⌥⌘V with a group: the copied effects onto every block of it that can carry them. */
export function pasteEffectsGroup() {
  withGroup((project, m) => {
    const { effects, projectId } = useClipboard.getState();
    if (!effects || projectId !== project.id) return;
    const ops = groupPasteEffectsOps(project, m, effects);
    commit(
      counted('timeline.group.pasteEffects', ops.length),
      ops,
      t({ id: 'timeline.group.effectsPasted', message: `Pasted effects onto ${ops.length} items` }),
    );
  });
}

/** The group's Mute: every sound of it muted (or back on), each keeping its volume. */
export function muteGroup(muted: boolean) {
  withGroup((project, m) => {
    const ops = groupMuteOps(project, m, muted);
    const count = ops.length;
    commit(
      counted(muted ? 'timeline.group.mute' : 'timeline.group.unmute', count),
      ops,
      muted
        ? t({ id: 'timeline.group.muted', message: `Muted ${count} items` })
        : t({ id: 'timeline.group.unmuted', message: `Unmuted ${count} items` }),
    );
  });
}

/** The group's Volume: every sound of it at `volume`. */
export function setGroupVolume(volume: number) {
  withGroup((project, m) => {
    const ops = groupVolumeOps(project, m, volume);
    const percent = Math.round(volume * 100);
    commit(
      counted('timeline.group.volume', ops.length),
      ops,
      t({ id: 'timeline.group.volumeSaid', message: `Volume ${percent} %` }),
    );
  });
}

/**
 * , and . with a group: its layers a frame earlier or later (⇧: a second), offsets kept; clips stay (the main track is
 * magnetic). Refused with a sentence when one can't move.
 */
export function nudgeGroup(direction: 1 | -1, large: boolean) {
  withGroup((project, m) => {
    if (m.layers.length === 0)
      return toast.info(t({ id: 'timeline.group.clipsStay', message: 'Clips on the Video track move one at a time.' }));
    const step = large ? 1_000_000 : frameUs(project.settings.fps);
    const ops = groupMoveOps(project, m, direction * step);
    if (typeof ops === 'string') return toast.info(moveRefusal(ops));
    commit(
      counted('timeline.group.move', ops.length),
      ops,
      t({ id: 'timeline.group.moved', message: `Moved ${ops.length} items` }),
    );
  });
}

/**
 * After a change (an undo, a redo, an edit), a group keeps the blocks that are still there: the one picked last stays
 * last when it is among them.
 */
export function forgetGoneBlocks(project: Project | undefined) {
  if (!project) return;
  const s = useSelection.getState();
  const ids = selectionOf(s);
  if (ids.length < 2) return;
  const there = new Set(everyBlock(project));
  const kept = ids.filter((id) => there.has(id));
  if (kept.length === ids.length) return;
  useSelection.getState().selectMany(kept, s.selectedId !== null && there.has(s.selectedId) ? s.selectedId : null);
}

/** ⌘A in the timeline: every clip and layer. */
export function selectEveryBlock() {
  const project = useProject.getState().snapshot?.project;
  if (!project) return;
  const ids = everyBlock(project);
  useSelection.getState().selectMany(ids);
  announce(t({ id: 'timeline.group.all', message: `Selected ${ids.length} items` }));
}

/**
 * ⇧-click on block `id`: the blocks from the one picked last to it, on one track, join the selection; on another
 * track, `id` joins alone.
 */
export function addRange(id: string) {
  const project = useProject.getState().snapshot?.project;
  const { selectedId } = useSelection.getState();
  if (!project) return;
  const ids = useSelection.getState().selectedIds;
  const from = selectedId !== null && !selectedId.includes(':') ? selectedId : null;
  const range = from === null ? null : rangeOnTrack(project, from, id);
  const before = from === null ? [] : ids.includes(from) ? ids : [from];
  useSelection.getState().selectMany([...before.filter((x) => !x.includes(':')), ...(range ?? [id])], id);
}
