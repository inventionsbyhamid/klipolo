import { plural, t } from '@lingui/core/macro';
import type { Project } from '../../../bindings/Project';
import type { LabelId } from '../../../lib/labels';
import { frameUs, snapToFrame } from '../../../lib/time';
import { useClipboard, type GroupEntry } from '../../../state/clipboard';
import { keyBurst } from '../../../state/keyBurst';
import { usePlayback } from '../../../state/playback';
import { useProject } from '../../../state/project';
import { groupOf, selectionOf, useSelection } from '../../../state/selection';
import { toast } from '../../../state/toasts';
import { useUi } from '../../../state/ui';
import { announce } from '../announce';
import { dragging } from '../drag/controller';
import { shiftBlock } from '../drag/registry';
import { usToPx } from '../scale';
import { paste } from './clipboardActions';
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

/** "3 items", "1 item": how many blocks a group holds, as its card's title and a screen reader say it. */
export function itemsWord(count: number): string {
  return t({
    id: 'timeline.group.items',
    message: plural(count, { one: '# item', other: '# items' }),
    comment: 'Several selected clips and layers',
  });
}

/** "Moves 3 clips together": a dragged group's clips, which go to their new place as one block. */
export function clipsTogether(count: number): string {
  return t({
    id: 'timeline.group.clipsTogether',
    message: plural(count, { one: 'Moves # clip together', other: 'Moves # clips together' }),
    comment: "Tip and screen-reader note while a group's clips are dragged: they go to the new place side by side",
  });
}

/** The sentence a refused group move says. */
export function moveRefusal(why: MoveRefusal): string {
  return why === 'locked'
    ? t({
        id: 'timeline.group.lockedMove',
        message: "A layer in the selection is on a locked track, so they can't move.",
      })
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
    const said = t({
      id: 'timeline.group.deleted',
      message: plural(count, { one: 'Deleted # item', other: 'Deleted # items' }),
    });
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
  // The effects copied before stay there for Paste effects: a group has no one stack of its own.
  const { effects, projectId } = useClipboard.getState();
  useClipboard.setState({
    projectId: project.id,
    entry: entries[0].entry,
    group: entries,
    effects: projectId === project.id ? effects : null,
  });
  const copied = countOf(m);
  announce(
    t({
      id: 'timeline.group.copied',
      message: plural(copied, { one: 'Copied # item', other: 'Copied # items' }),
    }),
  );
}

/** ⌘X with a group: copied, then deleted, as one step called Cut. */
export function cutGroup() {
  withGroup((project, m) => {
    const ops = groupDeleteOps(project, m);
    if (ops.length === 0) return;
    const entries = groupEntries(project, m);
    const count = deletedCount(m);
    // On the clipboard once the cut has gone in: a refused cut leaves it as it was.
    commit(
      counted('timeline.group.cut', count),
      ops,
      t({ id: 'timeline.group.cutSaid', message: plural(count, { one: 'Cut # item', other: 'Cut # items' }) }),
      () => {
        const { effects, projectId } = useClipboard.getState();
        useClipboard.setState({
          projectId: project.id,
          entry: entries[0].entry,
          group: entries,
          effects: projectId === project.id ? effects : null,
        });
        useSelection.getState().select(null);
      },
    );
  });
}

/** ⌘V of a copied group: at the playhead, its offsets kept; the copies are selected. */
export function pasteGroup(entries: GroupEntry[]) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const [ops, ids] = groupPasteOps(project, entries, usePlayback.getState().playheadUs);
    const pasted = ids.length;
    const said = t({
      id: 'timeline.group.pasted',
      message: plural(pasted, { one: 'Pasted # item', other: 'Pasted # items' }),
    });
    commit(counted('timeline.group.paste', ids.length), ops, said, () => useSelection.getState().selectMany(ids));
  });
}

/** ⌘D with a group: copies right after it, which are then selected. */
export function duplicateGroup() {
  withGroup((project, m) => {
    const [ops, ids] = groupDuplicateOps(project, m);
    if (ops.length === 0) return toast.info(t({ id: 'timeline.trackLocked', message: 'This track is locked.' }));
    const copies = ids.length;
    const said = t({
      id: 'timeline.group.duplicated',
      message: plural(copies, { one: 'Duplicated # item', other: 'Duplicated # items' }),
    });
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
      t({
        id: 'timeline.group.effectsPasted',
        message: plural(ops.length, { one: 'Pasted effects onto # item', other: 'Pasted effects onto # items' }),
      }),
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
        ? t({ id: 'timeline.group.muted', message: plural(count, { one: 'Muted # item', other: 'Muted # items' }) })
        : t({
          id: 'timeline.group.unmuted',
          message: plural(count, { one: 'Unmuted # item', other: 'Unmuted # items' }),
        }),
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

/** The blocks a settling group nudge has drawn moved. */
const nudged = new Set<string>();

/** One press of a group nudge: it adds to the presses of a held key settling as one step (`keyBurst`). */
function pressGroup(direction: 1 | -1, large: boolean, key: string, quiet: boolean) {
  const project = editable();
  const m = project ? selectedGroup(project) : null;
  if (!project || !m || countOf(m) === 0) return;
  if (m.layers.length === 0) {
    if (!quiet)
      toast.info(
        t({
          id: 'timeline.group.clipsStay',
          message: 'Clips keep their places on the Video track. Drag them to move them.',
        }),
      );
    return;
  }
  const fps = project.settings.fps;
  const before = keyBurst.pending(key)?.total ?? 0;
  // On the frame grid, whatever mix of frames and seconds the presses make.
  const total = snapToFrame(before + direction * (large ? 1_000_000 : frameUs(fps)), fps);
  const ops = groupMoveOps(project, m, total);
  if (typeof ops === 'string') {
    if (!quiet) toast.info(moveRefusal(ops));
    return;
  }
  // Drawn moved at once, as a one-block nudge is; the commit lands after the burst.
  const px = usToPx(total, useUi.getState().pxPerSecond);
  for (const { item } of m.layers) {
    shiftBlock(item.id, px);
    nudged.add(item.id);
  }
  const count = ops.length;
  keyBurst.push(counted('timeline.group.move', count), ops, key, {
    total,
    done: (ok) => {
      for (const id of nudged) shiftBlock(id, 0);
      nudged.clear();
      if (ok && count > 0)
        announce(
          t({
            id: 'timeline.group.moved',
            message: plural(count, { one: 'Moved # item', other: 'Moved # items' }),
          }),
        );
    },
  });
}

/**
 * , and . (and ← / →) with a group: its layers a frame earlier or later (⇧: a second), offsets kept; clips stay (the
 * main track is magnetic). A held key's presses add up into one undo step, as a one-block nudge's do. Refused with a
 * sentence when one can't move, once per held key (`quiet`: a repeat).
 */
export function nudgeGroup(direction: 1 | -1, large: boolean, quiet = false) {
  // A drag owns the timeline until it lets go.
  if (dragging()) return;
  const key = 'nudge:group';
  if (keyBurst.pending(key)) pressGroup(direction, large, key, quiet);
  else keyBurst.after(() => pressGroup(direction, large, key, quiet), key);
}

/** "Selected 3 items": what a selection made from the keyboard holds. */
export function selectedWord(count: number): string {
  return t({
    id: 'timeline.group.all',
    message: plural(count, { one: 'Selected # item', other: 'Selected # items' }),
  });
}

/** ⇧Enter on a focused block: it joins the selection, or leaves it, and the selection's size is said. */
export function toggleBlock(id: string) {
  useSelection.getState().toggle(id);
  announce(selectedWord(selectionOf(useSelection.getState()).length));
}

/** Paste: a copied group, else the one block copied. */
export function pasteClipboard() {
  const group = useClipboard.getState().group;
  if (group && group.length > 1) pasteGroup(group);
  else paste();
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
  announce(selectedWord(ids.length));
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
