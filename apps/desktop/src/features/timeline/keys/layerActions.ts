import { t } from '@lingui/core/macro';
import { labelText } from '../../../lib/labels';
import { musicRunOf } from '../../../lib/musicRuns';
import { cameraSource } from '../../../lib/recording';
import { clipAt, findItem, itemLabel, sourceLabel } from '../../../lib/timeline';
import { keyBurst } from '../../../state/keyBurst';
import { useProject } from '../../../state/project';
import { toast } from '../../../state/toasts';
import { itemOp, muteLabel } from '../../preview';
import { attachOps, trackStepOps } from './keyOps';
import { blockLabel, commit, editable } from './timelineActions';
import { setTrackFlag } from './trackActions';

/*
 * A layer's track, clip and mute, and a recording's camera (`useTimelineCommands`): each built like `timelineActions`'
 * edits, from the project when it runs, as one undo step through `useProject.apply`, said in the live region.
 */

/** ⌥↑ / ⌥↓: layer `id` onto the track above (1) or below (−1). */
export function stepTrack(id: string, direction: 1 | -1) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const ops = trackStepOps(project, id, direction);
    const op = ops[0];
    const to =
      op?.type === 'moveItem' ? project.timeline.tracks.find((tr) => tr.id === op.trackId) : undefined;
    const track = to?.name ?? '';
    const label = blockLabel(project, id);
    const said = t({
      id: 'timeline.key.trackMoved',
      message: `Moved ${label} to ${track}`,
      comment: '{track} is a track’s name, e.g. Overlay 2',
    });
    commit(direction > 0 ? 'undo.trackUp' : 'undo.trackDown', ops, said);
  });
}

/** Keep with clip / Free from clip on layer `id`. */
export function toggleAttach(id: string) {
  keyBurst.after(() => {
    const project = editable();
    const item = project ? findItem(project, id)?.item : undefined;
    if (!project || !item) return;
    const label = itemLabel(project, item);
    // It lets go of its clip, or attaches to the clip under its start.
    const source = item.attach?.source ?? clipAt(project.timeline.main, item.startUs)?.clip.source;
    const clip = source ? sourceLabel(project, source) : '';
    const said = item.attach
      ? t({
          id: 'timeline.key.detached',
          message: `${label} no longer moves with ${clip}`,
          comment: 'Screen-reader note; {clip} is a clip’s name',
        })
      : t({
          id: 'timeline.key.attached',
          message: `${label} now moves with ${clip}`,
          comment: 'Screen-reader note; {clip} is a clip’s name',
        });
    commit(item.attach ? 'undo.attach.free' : 'undo.attach.keep', attachOps(item), said);
  });
}

/** Separate camera: recording clip `clipId`'s camera becomes its own layer (`detachSource` of its camera source). */
export function separateCamera(clipId: string) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const clip = project.timeline.main.find((c) => c.id === clipId);
    const source = clip?.source;
    const recording =
      source?.type === 'recording' ? project.recordings.find((r) => r.id === source.id) : undefined;
    const camera = recording ? cameraSource(recording) : undefined;
    if (!camera) return;
    const label = blockLabel(project, clipId);
    const said = t({
      id: 'timeline.key.cameraSeparated',
      message: `The camera of ${label} is now its own layer`,
    });
    commit('undo.separateCamera', [{ type: 'detachSource', clipId, sourceId: camera.id }], said);
  });
}

/**
 * Mute or Unmute on sound layer `id`: its own switch (the inspector's Mute), so its volume stays for when it plays
 * again. A song laid in pieces back to back (`musicRunOf`) is muted or unmuted whole, in one step, as the piece pressed
 * says: no piece plays on where the others went quiet. Another kind of layer is left alone.
 */
export function toggleSoundMute(id: string) {
  keyBurst.after(() => {
    const project = editable();
    const item = project ? findItem(project, id)?.item : undefined;
    const content = item?.content;
    if (!project || !item || content?.type !== 'audio') return;
    const muted = !content.muted;
    const label = itemLabel(project, item);
    const said = muted
      ? t({
          id: 'timeline.key.soundMuted',
          message: `Muted ${label}`,
          comment: 'Screen-reader note; {label} is a sound layer’s name',
        })
      : t({
          id: 'timeline.key.soundUnmuted',
          message: `Unmuted ${label}`,
          comment: 'Screen-reader note; {label} is a sound layer’s name',
        });
    const pieces = musicRunOf(project.timeline.tracks, id) ?? [item];
    const ops = pieces.flatMap((piece) =>
      piece.content.type === 'audio' ? [itemOp(piece, { content: { ...piece.content, muted } })] : [],
    );
    commit(muteLabel(muted, 'sound'), ops, said, () => {
      if (!muted) noteMutedTrack(id);
    });
  });
}

/**
 * After sound layer `id` is unmuted: on a muted track it still can't be heard, so a note says why (once per track:
 * keyed), with Unmute track, one undo step as the track's header does.
 */
export function noteMutedTrack(id: string) {
  const project = useProject.getState().snapshot?.project;
  const found = project ? findItem(project, id) : null;
  if (!found?.track.muted) return;
  const { track } = found;
  const name = track.name;
  const text = t({
    id: 'timeline.sound.trackMuted',
    message: `The ${name} track is muted.`,
    comment:
      'After a sound layer is unmuted, it stays silent: its track is muted. {name} is the track’s name',
  });
  toast.info(text, {
    key: `track-muted:${track.id}`,
    actions: [
      { label: labelText('timeline.track.unmute'), run: () => setTrackFlag(track.id, 'muted', false) },
    ],
  });
}
