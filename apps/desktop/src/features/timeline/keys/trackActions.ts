import { plural, select, t } from '@lingui/core/macro';
import { create } from 'zustand';
import type { Track } from '../../../bindings/Track';
import type { TrackKind } from '../../../bindings/TrackKind';
import { newId } from '../../../lib/defaults';
import { labelText } from '../../../lib/labels';
import { findItem, itemLabel } from '../../../lib/timeline';
import { keyBurst } from '../../../state/keyBurst';
import { useProject } from '../../../state/project';
import { toast } from '../../../state/toasts';
import { useUi } from '../../../state/ui';
import { laneFlag, laneTracks, setLaneOps, type LaneId } from '../model/laneOps';
import { moveItemOps } from '../model/moveOps';
import {
  addTrackOps,
  moveTrackOps,
  removeTrackOps,
  renameTrackOps,
  setTrackOps,
  soloOffOps,
  trackFlag,
  type TrackToggle,
} from '../model/trackOps';
import { announce } from '../announce';
import { beginCommit } from '../../../state/commitQueue';
import { commit, editable } from './timelineActions';
import { laneName, laneToggleLabel, toggledSaid, trackToggleId } from './trackLabels';

/*
 * What the track and lane commands do (a track's header and options, a lane's, a sound layer's track muted, a layer's
 * own track): each reads the project when it runs, builds its ops with `model/trackOps` or `model/laneOps`, commits
 * them as one undo step through `useProject.apply` in the timeline's commit queue, and says what it did in the live
 * region.
 */

/** The track whose name is a field in its header now (Rename), if any. */
export const useTrackRename = create<{ id: string | null }>(() => ({ id: null }));

/** Track `id`'s name becomes a field in its header: every track shows, so its header does. */
export function startRename(id: string) {
  useUi.getState().setTimelineExpanded(true);
  useTrackRename.setState({ id });
}

// A name being edited is dropped when the headers go (fewer tracks shown, another project): it never reopens later.
const endRename = () => {
  if (useTrackRename.getState().id !== null) useTrackRename.setState({ id: null });
};
useUi.subscribe((u, prev) => {
  if (prev.timelineExpanded && !u.timelineExpanded) endRename();
});
useProject.subscribe((p, prev) => {
  if (p.snapshot?.project.id !== prev.snapshot?.project.id) endRename();
});

const trackIn = (id: string) => {
  const project = editable();
  const track = project?.timeline.tracks.find((tr) => tr.id === id);
  return project && track ? { project, track } : null;
};

const locked = () => toast.info(t({ id: 'timeline.trackLocked', message: 'This track is locked.' }));

/** Add overlay track / Add audio track: a new, empty one, which the model names and places. */
export function addTrack(kind: TrackKind) {
  keyBurst.after(() => {
    if (!editable()) return;
    const said = t({
      id: 'timeline.track.added',
      message: select(kind, {
        audio: 'Added an audio track',
        zoom: 'Added a zoom track',
        other: 'Added an overlay track',
      }),
    });
    commit('timeline.track.add', addTrackOps(kind), said);
  });
}

/** Track `id` renamed to `name` (trimmed); an empty name, or the one it has, changes nothing. */
export function renameTrack(id: string, name: string) {
  keyBurst.after(() => {
    const found = trackIn(id);
    if (!found) return;
    const ops = renameTrackOps(found.track, name);
    const from = found.track.name;
    const to = ops[0]?.type === 'updateTrack' ? (ops[0].patch.name ?? '') : '';
    commit(
      'timeline.track.rename',
      ops,
      t({ id: 'timeline.track.renamed', message: `Renamed ${from} to ${to}` }),
    );
  });
}

/** Track `id` one row up (1) or down (−1) among its own rows (picture tracks, or audio tracks). */
export function moveTrack(id: string, direction: 1 | -1) {
  keyBurst.after(() => {
    const found = trackIn(id);
    if (!found) return;
    const name = found.track.name;
    commit(
      'timeline.track.move',
      moveTrackOps(found.project.timeline.tracks, id, direction),
      direction > 0
        ? t({ id: 'timeline.track.movedUp', message: `Moved ${name} up` })
        : t({ id: 'timeline.track.movedDown', message: `Moved ${name} down` }),
    );
  });
}

/**
 * Turn off solo: every soloed track plays again, as one undo step (the export's "Export everything" runs it first).
 * Resolves true once it has gone in (or there was nothing soloed).
 */
export function soloOff(): Promise<boolean> {
  return new Promise((resolve) =>
    keyBurst.after(() => {
      const project = editable();
      const ops = project ? soloOffOps(project.timeline.tracks) : [];
      if (!project || ops.length === 0) return resolve(project !== null);
      const end = beginCommit();
      // At its turn, holding the commit queue (`beginCommit`), as `timelineActions.commit` does.
      // eslint-disable-next-line no-restricted-syntax -- sent at its turn inside `keyBurst.after`
      void useProject
        .getState()
        .apply('timeline.track.soloOff', ops)
        .then((ok) => {
          if (ok)
            announce(t({ id: 'timeline.track.soloOff.said', message: 'Solo is off: every track plays' }));
          resolve(ok);
        })
        .finally(end);
    }),
  );
}

/** Turn off solo's command name, which is also its undo label. */
export const soloOffLabel = () => labelText('timeline.track.soloOff');

/**
 * Hide or show, mute or unmute, solo or unsolo, lock or unlock track `id`: `value` is the state it ends in (true:
 * hidden, muted, locked); without one, the other state from the one the track has at its turn, so two quick presses of
 * a header's toggle hide it and show it again. A field that doesn't apply to the track's kind
 * (muting a picture track) does nothing.
 */
export function setTrackFlag(id: string, field: TrackToggle, to?: boolean) {
  keyBurst.after(() => {
    const found = trackIn(id);
    if (!found) return;
    const { track } = found;
    const value = to ?? !trackFlag(track, field);
    const said = toggledSaid(track.name, field, value);
    // The undo entry says what changed, as the button that does it says it ("Hide track").
    commit(trackToggleId(track, field), setTrackOps(track, field, value), said);
  });
}

/**
 * Hide or show, mute or unmute, solo or unsolo, lock or unlock every track lane `lane` draws (its header and options),
 * as one undo step: `value` is the state they end in; without one, the other state from the lane's at its turn (on
 * when not every track has it). A lane without a track `field` applies to does nothing.
 */
export function setLaneFlag(lane: LaneId, field: TrackToggle, to?: boolean) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const on = laneTracks(project.timeline.tracks, lane);
    const value = to ?? !laneFlag(on, field);
    const ops = setLaneOps(on, field, value);
    commit(laneToggleLabel(lane, field, value), ops, toggledSaid(laneName(lane), field, value));
  });
}

/** The tracks in the order their rows draw: picture tracks top first, then audio tracks. */
function rowOrder(tracks: Track[]): Track[] {
  return [
    ...tracks.filter((tr) => tr.kind !== 'audio').reverse(),
    ...tracks.filter((tr) => tr.kind === 'audio'),
  ];
}

/**
 * Delete track: track `id` and its layers out (a removal: the toast says what went and how to get it back, and one
 * undo step brings both back). A locked track says so. Focus that was in its row goes to the next row's options.
 */
export function deleteTrack(id: string) {
  keyBurst.after(() => {
    const found = trackIn(id);
    if (!found) return;
    const { project, track } = found;
    if (track.locked) return locked();
    const name = track.name;
    const count = track.items.length;
    const rows = rowOrder(project.timeline.tracks);
    const at = rows.findIndex((tr) => tr.id === id);
    const next = rows[at + 1] ?? rows[at - 1];
    // Said once: a track with layers in it by the toast, which also says how to get them back (its region is read
    // out); an empty one in the timeline's live region.
    const said = count > 0 ? '' : t({ id: 'timeline.track.deleted', message: `Deleted ${name}` });
    commit('timeline.track.delete', removeTrackOps(track), said, () => {
      if (count > 0)
        toast.info(
          t({
            id: 'timeline.track.deletedRisk',
            message: plural(count, {
              one: `Deleted ${name} and its # layer. ⌘Z brings them back.`,
              other: `Deleted ${name} and its # layers. ⌘Z brings them back.`,
            }),
            comment: 'A toast after deleting a track that had layers in it; ⌘Z is Undo',
          }),
        );
      const active = document.activeElement;
      if (active === null || active === document.body || active.closest(`[data-track-id="${id}"]`))
        document
          .querySelector<HTMLElement>(next ? `[data-track-menu="${next.id}"]` : '[data-add-track] button')
          ?.focus({ preventScroll: true });
    });
  });
}

/** Move to a new track: layer `itemId` onto a new track of its kind (on top of its rows), where it is in time. */
export function moveToNewTrack(itemId: string) {
  keyBurst.after(() => {
    const project = editable();
    const found = project ? findItem(project, itemId) : null;
    if (!project || !found) return;
    if (found.track.locked) return locked();
    const { item, track } = found;
    const index = project.timeline.tracks.length;
    const ops = moveItemOps(item, track.id, track.id, item.startUs, { id: newId(), kind: track.kind, index });
    const label = itemLabel(project, item);
    commit(
      'timeline.move.layerNewTrack.undo',
      ops,
      t({ id: 'timeline.track.layerMoved', message: `Moved ${label} to a new track` }),
    );
  });
}
