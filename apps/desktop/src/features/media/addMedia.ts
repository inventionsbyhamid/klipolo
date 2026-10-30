import { t } from '@lingui/core/macro';
import type { AssetKind } from '../../bindings/AssetKind';
import type { Op } from '../../bindings/Op';
import type { Project } from '../../bindings/Project';
import { activeIntlLocale } from '../../i18n';
import { formatNumber } from '../../lib/format';
import { formatTime } from '../../lib/time';
import { keyBurst } from '../../state/keyBurst';
import { usePlayback } from '../../state/playback';
import { useSelection } from '../../state/selection';
import { toast } from '../../state/toasts';
import { commit, editable } from '../timeline/keys/timelineActions';
import { dropOps, type DropTarget } from '../timeline/model/dropOps';

/*
 * What adding media to the timeline does, from the library (a row dragged onto the timeline, a row's menu): it reads
 * the project when it runs, after any commit in flight has landed (the timeline's commit queue), builds its ops with
 * `dropOps`, commits them as one undo step through `useProject.apply`, says what it did, and selects what it added.
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
  if (op?.type !== 'addItem') return '';
  const time = formatTime(op.item.startUs);
  return op.item.content.type === 'audio'
    ? t({
        id: 'media.added.sound',
        message: `Added ${name} as a sound at ${time}`,
        comment: 'Screen-reader note; {name} is a file name, {time} a time in the video, e.g. 1:05',
      })
    : t({
        id: 'media.added.layer',
        message: `Added ${name} as a layer at ${time}`,
        comment: 'Screen-reader note; {name} is a file name, {time} a time in the video, e.g. 1:05',
      });
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
};

/**
 * Asset `assetId` onto the timeline at `target` (or where `target` says, worked out from the project when it runs). A
 * file the project doesn't have, or one too short to play, says so; a read-only project or a block being dragged
 * takes no edit, as for every timeline action.
 */
export function addMedia(assetId: string, target: DropTarget | ((project: Project) => DropTarget)) {
  keyBurst.after(() => {
    const project = editable();
    if (!project) return;
    const asset = project.assets.find((a) => a.id === assetId);
    if (!asset) return toast.info(mediaRefusal.unknown());
    const at = typeof target === 'function' ? target(project) : target;
    const ops = dropOps(project, asset, at, project.settings.fps);
    if (ops.length === 0) return toast.info(mediaRefusal.tooShort());
    const placed = ops.at(-1);
    const id =
      placed?.type === 'insertClip' ? placed.clip.id : placed?.type === 'addItem' ? placed.item.id : null;
    commit('media.add', ops, addedSaid(project, placed, asset.name), () => useSelection.getState().select(id));
  });
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
  addMedia(assetId, (project) => ({ lane: 'main', index: project.timeline.main.length }));
}

/** Add as layer at playhead: a video or image as a picture layer, attached to the clip there. */
export function addLayerAtPlayhead(assetId: string) {
  addMedia(assetId, () => ({ lane: 'picture', trackId: null, atUs: usePlayback.getState().playheadUs }));
}

/** Add sound at playhead: a sound, free of the clips. */
export function addSoundAtPlayhead(assetId: string) {
  addMedia(assetId, () => ({ lane: 'sound', trackId: null, atUs: usePlayback.getState().playheadUs }));
}
