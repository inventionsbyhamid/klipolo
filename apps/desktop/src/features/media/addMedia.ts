import { t } from '@lingui/core/macro';
import type { AssetKind } from '../../bindings/AssetKind';
import type { CameraLayout } from '../../bindings/CameraLayout';
import type { ClipSource } from '../../bindings/ClipSource';
import type { Op } from '../../bindings/Op';
import type { Project } from '../../bindings/Project';
import type { Recording } from '../../bindings/Recording';
import { activeIntlLocale } from '../../i18n';
import { cameraLayoutFor, newRecordingClip } from '../../lib/defaults';
import { formatNumber } from '../../lib/format';
import { cameraSource, primarySource } from '../../lib/recording';
import { recordingName } from '../../lib/timeline';
import { keyBurst } from '../../state/keyBurst';
import { usePlayback } from '../../state/playback';
import { useSelection } from '../../state/selection';
import { toast } from '../../state/toasts';
import { commit, dropOps, editable, type DropTarget } from '../timeline/drops';
import { addMusic, dropUs, isMusicTarget, layerSaid, placeAt } from './place';

export { isMusicTarget };

/*
 * What adding media to the timeline does, from the library (a row dragged onto the timeline, a row's menu): it reads
 * the project when it runs, after any commit in flight has landed (the timeline's commit queue). A sound or a picture
 * Rust places (`place.ts`: music that fits the video on the Music lane, else as the "+" places it, on the track it was
 * let go on); a video goes in by `dropOps` (a clip of the main track, or a layer). One undo step either way, said, and
 * what it added selected.
 */

/** What an add says once it has gone in: where `name` went. */
function addedSaid(project: Project, op: Op | undefined, name: string): string {
  if (op?.type === 'insertClip') {
    const position = formatNumber(activeIntlLocale(), (op.index ?? project.timeline.main.length) + 1);
    return t({
      id: 'media.added.clip',
      message: `Added ${name} to the main track at position ${position}`,
      comment: 'Screen-reader note; {name} is a file name, {position} a number (1 is the first clip)',
    });
  }
  return op?.type === 'addItem' ? layerSaid(op.item, name) : '';
}

/** Why an add can't happen, as the user (or chat, through the toast) hears it. */
export const mediaRefusal = {
  noFile: () => t({ id: 'media.add.noFile', message: 'Choose a file to add.' }),
  unknown: () =>
    t({
      id: 'media.add.unknown',
      message: "That file isn't in this project.",
      comment: 'A media command named a file that was never imported, or was removed',
    }),
  noTarget: () => t({ id: 'media.add.noTarget', message: 'Choose where to add the file.' }),
  tooShort: () => t({ id: 'media.add.tooShort', message: 'This file is too short to add.' }),
  tooShortMany: () => t({ id: 'media.add.tooShortMany', message: 'These files are too short to add.' }),
  recordingUnknown: () =>
    t({
      id: 'media.add.recordingUnknown',
      message: "That recording isn't in this project.",
      comment: 'The library added a recording that was removed meanwhile',
    }),
  recordingTooShort: () => t({ id: 'media.add.recordingTooShort', message: 'This recording is too short to add.' }),
};

/**
 * Asset `assetId` onto the timeline at `target` (or where `target` says, worked out from the project when it runs). A
 * file the project doesn't have, or one too short to play, says so; a read-only project or a block being dragged
 * takes no edit, as for every timeline action. `added` runs once it has gone in.
 */
export function addMedia(
  assetId: string,
  target: DropTarget | ((project: Project) => DropTarget),
  added?: () => void,
) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const asset = project.assets.find((a) => a.id === assetId);
    if (!asset) return toast.info(mediaRefusal.unknown());
    const at = typeof target === 'function' ? target(project) : target;
    const source = { type: 'asset', id: asset.id } as const;
    const then = (ok: boolean) => void (ok && added?.());
    if (asset.kind === 'audio' && isMusicTarget(at)) return void addMusic(source, dropUs(project, at)).then(then);
    if (asset.kind !== 'video') return void placeAt([source], at).then(then);
    const ops = dropOps(project, asset, at, project.settings.fps);
    if (ops.length === 0) return toast.info(mediaRefusal.tooShort());
    const placed = ops.at(-1);
    const id =
      placed?.type === 'insertClip' ? placed.clip.id : placed?.type === 'addItem' ? placed.item.id : null;
    commit('media.add', ops, addedSaid(project, placed, asset.name), () => {
      useSelection.getState().select(id);
      added?.();
    });
  });
}

/**
 * The camera a new clip of `recording` plays its camera with: as the video's latest clip of it does (the person's
 * bubble and mirror), else as a take lands (`cameraLayoutFor`); none for a take without a camera. With no clip of the
 * take left on the video the camera is mirrored, as by default: the take's own "Mirror my camera" is kept only in its
 * folder, which the project doesn't hold, so a take recorded unmirrored comes back mirrored (its card's Mirror turns it
 * back).
 */
function recordingCamera(project: Project, recording: Recording): CameraLayout | null {
  const main = project.timeline.main;
  for (let i = main.length - 1; i >= 0; i--) {
    const { source, camera } = main[i];
    if (source.type === 'recording' && source.id === recording.id) return camera;
  }
  const cameraOnly = primarySource(recording)?.role === 'camera';
  return cameraSource(recording) || cameraOnly ? cameraLayoutFor(project.settings.primaryFormat) : null;
}

/**
 * Puts the recording `recordingId`, the whole take, in the main track at `target`'s cut (or where `target` says, worked
 * out from the project when it runs): one undo step, said, and the new clip selected. It goes on the Video lane only
 * (its drag lands nowhere else). A recording the project doesn't have says so; a read-only project or a block being
 * dragged takes no edit. `added` runs once it has gone in.
 */
export function addRecording(
  recordingId: string,
  target: DropTarget | ((project: Project) => DropTarget),
  added?: () => void,
) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const recording = project.recordings.find((r) => r.id === recordingId);
    if (!recording) return toast.info(mediaRefusal.recordingUnknown());
    const at = typeof target === 'function' ? target(project) : target;
    if (at.lane !== 'main') return;
    const clip = newRecordingClip(recording, recordingCamera(project, recording));
    if (clip.sourceOutUs <= 0) return toast.info(mediaRefusal.recordingTooShort());
    const main = project.timeline.main;
    const op: Op = { type: 'insertClip', clip, index: Math.max(0, Math.min(main.length, at.index)) };
    commit('media.add', [op], addedSaid(project, op, recordingName(project, recording.id)), () => {
      useSelection.getState().select(clip.id);
      added?.();
    });
  });
}

/** After the last clip of the main track, as the project is when the add runs. */
export const toTheEnd = (project: Project): DropTarget => ({ lane: 'main', index: project.timeline.main.length });

/** A Clips item onto the timeline at `target`: a file (`addMedia`) or a whole recording (`addRecording`). */
export function addClip(
  source: ClipSource,
  target: DropTarget | ((project: Project) => DropTarget),
  added?: () => void,
) {
  if (source.type === 'recording') addRecording(source.id, target, added);
  else addMedia(source.id, target, added);
}

/**
 * Which of the row menu's adds suit a file of `kind` (the menu and ⌘K offer only those): only a video makes a clip of
 * the main track, a video or a picture a layer, and a sound a sound layer.
 */
export const mediaFits = {
  main: (kind: AssetKind) => kind === 'video',
  layer: (kind: AssetKind) => kind !== 'audio',
  sound: (kind: AssetKind) => kind === 'audio',
};

/** Add to main track: a video after the last clip. */
export function addToMain(assetId: string) {
  addMedia(assetId, toTheEnd);
}

/** Add as layer at playhead: a video or image as a picture layer, attached to the clip there. */
export function addLayerAtPlayhead(assetId: string) {
  addMedia(assetId, () => ({ lane: 'picture', trackId: null, atUs: usePlayback.getState().playheadUs }));
}
