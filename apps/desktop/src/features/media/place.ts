import { t } from '@lingui/core/macro';
import type { DropOnto } from '../../bindings/DropOnto';
import type { MediaSource } from '../../bindings/MediaSource';
import type { Item } from '../../bindings/Item';
import type { PackItem } from '../../bindings/PackItem';
import type { Project } from '../../bindings/Project';
import type { SnapshotPatch } from '../../bindings/SnapshotPatch';
import type { TextKind } from '../../bindings/TextKind';
import { library } from '../../bridge/library';
import { formatTime } from '../../lib/time';
import { clipStarts, mainDurationUs } from '../../lib/timeline';
import { queueCommand } from '../../state/commit';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import { announce } from '../timeline/announce';
import { layerStartUs, type DropTarget } from '../timeline/drops';

/*
 * What the library's "+" and drops on the Music lane do, and the Add title menu: Rust builds the layer from the project
 * as it is (`bridge/library`: music that fits the video, a sound effect, a picture, a title), as one undo step through
 * the commit queue, and the new layer is selected once it is in.
 */

/** Every layer's id in `project`. */
function layerIds(project: Project | undefined): Set<string> {
  return new Set(project?.timeline.tracks.flatMap((t) => t.items.map((i) => i.id)) ?? []);
}

/** What screen readers hear once `item` is in, `name` being its file's (or its words'). */
export function layerSaid(item: Item, name: string): string {
  const time = formatTime(item.startUs);
  const c = item.content;
  if (c.type === 'audio' && typeof c.duck === 'number')
    return t({
      id: 'media.added.music',
      message: `Added ${name} as music from ${time}`,
      comment: 'Screen-reader note; {name} is a file name, {time} a time in the video, e.g. 1:05',
    });
  return c.type === 'audio'
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

/** The name screen readers hear for `item`: its file's, its words, or its template's. */
function nameOf(project: Project, item: Item): string {
  const c = item.content;
  if ('assetId' in c) return project.assets.find((a) => a.id === c.assetId)?.name ?? '';
  if (c.type === 'text') return c.text;
  if (c.type === 'generator') return c.source.endsWith('lower-third') ? templateName.lowerThird() : templateName.titleCard();
  return '';
}

/**
 * Sends `task` at its turn, says what it added, and selects the first layer it added (music's first piece). The
 * layers there before are read at its turn, after the commits ahead of it landed, so none of theirs counts as its own.
 * Resolves with whether it went in.
 */
async function placing(task: () => Promise<SnapshotPatch>): Promise<boolean> {
  let before = new Set<string>();
  const ok = await queueCommand(() => {
    before = layerIds(useProject.getState().snapshot?.project);
    return task();
  });
  if (!ok) return false;
  const project = useProject.getState().snapshot?.project;
  const added = project?.timeline.tracks
    .flatMap((t) => t.items)
    .filter((i) => !before.has(i.id))
    .sort((a, b) => a.startUs - b.startUs);
  if (project && added?.length) {
    announce(layerSaid(added[0], nameOf(project, added[0])));
    useSelection.getState().select(added[0].id);
  }
  return true;
}

/**
 * `source` as background music to the video's end, ducking under speech: under the whole video (the "+", as the Music
 * tab says), or from `atUs` where it was let go on the Music lane, tiled to the end.
 */
export function addMusic(source: MediaSource, atUs = 0): Promise<boolean> {
  return placing(() => library.addMusic(source, Math.max(0, atUs)));
}

/** `source` as a sound effect at `atUs` (the playhead when left out). */
export function addSound(source: MediaSource, atUs?: number): Promise<boolean> {
  const at = atUs ?? usePlayback.getState().playheadUs;
  return placing(() => library.addSound(source, at));
}

/** Picture `source` at `atUs` (the playhead when left out): medium, centred, for 5 s. */
export function addImage(source: MediaSource, atUs?: number): Promise<boolean> {
  const at = atUs ?? usePlayback.getState().playheadUs;
  return placing(() => library.addImage(source, at));
}

/** A title template or text preset at the playhead. */
export function addTitle(kind: TextKind): Promise<boolean> {
  const at = usePlayback.getState().playheadUs;
  return placing(() => library.addText(kind, at));
}

/** The name the Add title menu and the inspector give each title template. */
export const templateName = {
  lowerThird: () => t({ id: 'title.template.lowerThird', message: 'Lower third' }),
  titleCard: () => t({ id: 'title.template.titleCard', message: 'Title card' }),
};

/**
 * Where a drop at `target` starts, as its ghost shows (`DropIndicator`): a cut's time on the main track, else the time
 * under the pointer on the frame grid, or exactly on a cut or a layer's edge it was let go on.
 */
export function dropUs(project: Project, target: DropTarget): number {
  if (target.lane === 'main') {
    const starts = clipStarts(project.timeline.main);
    return target.index < starts.length ? starts[target.index] : mainDurationUs(project.timeline.main);
  }
  return layerStartUs(project, target.atUs, project.settings.fps);
}

/** The track a drop at `target` names: the one it was let go on, a new one, or none. */
export function dropOnto(target: DropTarget): DropOnto {
  if (target.lane === 'main') return { type: 'auto' };
  if (target.newTrack) return { type: target.lane === 'sound' ? 'newSound' : 'newPicture' };
  return target.trackId === null ? { type: 'auto' } : { type: 'track', id: target.trackId };
}

/** Whether a drop at `target` adds music: a sound let go on the Music lane. */
export function isMusicTarget(target: DropTarget): target is Extract<DropTarget, { lane: 'sound' }> {
  return target.lane === 'sound' && target.row === 'music';
}

/**
 * Sounds and pictures let go at `target`: each placed as the "+" places it (attached to the clip under it, cut at the
 * video's end, a picture medium and centred), one after another, on the track it was let go on (`library.place`).
 */
export function placeAt(sources: MediaSource[], target: DropTarget): Promise<boolean> {
  const project = useProject.getState().snapshot?.project;
  if (!project) return Promise.resolve(false);
  return placing(() => library.place(sources, dropUs(project, target), dropOnto(target)));
}

/** A pack item let go on the timeline: music on the Music lane, else placed where it was let go. */
export function dropPackItem(item: PackItem, target: DropTarget): Promise<boolean> {
  const project = useProject.getState().snapshot?.project;
  if (!project) return Promise.resolve(false);
  const source: MediaSource = { type: 'pack', id: item.id };
  if (item.category !== 'sticker' && isMusicTarget(target)) return addMusic(source, dropUs(project, target));
  return placeAt([source], target);
}

/** A pack item at the playhead: music under the video, a sound effect, or a sticker. */
export function addPackItem(item: PackItem): Promise<boolean> {
  const source: MediaSource = { type: 'pack', id: item.id };
  if (item.category === 'music') return addMusic(source);
  if (item.category === 'sound') return addSound(source);
  return addImage(source);
}
