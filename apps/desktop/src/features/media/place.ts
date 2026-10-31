import { t } from '@lingui/core/macro';
import type { MediaSource } from '../../bindings/MediaSource';
import type { PackItem } from '../../bindings/PackItem';
import type { Project } from '../../bindings/Project';
import type { SnapshotPatch } from '../../bindings/SnapshotPatch';
import type { TextKind } from '../../bindings/TextKind';
import { library } from '../../bridge/library';
import { clipStarts, mainDurationUs, videoDurationUs } from '../../lib/timeline';
import { queueCommand } from '../../state/commit';
import { usePlayback } from '../../state/playback';
import { useProject } from '../../state/project';
import { useSelection } from '../../state/selection';
import type { DropTarget } from '../timeline/drops';

/*
 * What the library's "+" and drops on the Music lane do, and the Add title menu: Rust builds the layer from the project
 * as it is (`bridge/library`: music that fits the video, a sound effect, a picture, a title), as one undo step through
 * the commit queue, and the new layer is selected once it is in.
 */

/** Every layer's id in `project`. */
function layerIds(project: Project | undefined): Set<string> {
  return new Set(project?.timeline.tracks.flatMap((t) => t.items.map((i) => i.id)) ?? []);
}

/**
 * Sends `task` at its turn and selects the first layer it added (music's first piece). Resolves with whether it went
 * in.
 */
async function placing(task: () => Promise<SnapshotPatch>): Promise<boolean> {
  const before = layerIds(useProject.getState().snapshot?.project);
  const ok = await queueCommand(task);
  if (!ok) return false;
  const project = useProject.getState().snapshot?.project;
  const added = project?.timeline.tracks
    .flatMap((t) => t.items)
    .filter((i) => !before.has(i.id))
    .sort((a, b) => a.startUs - b.startUs)[0];
  if (added) useSelection.getState().select(added.id);
  return true;
}

/**
 * Where music added at `atUs` starts: there, while the video goes on after it; from the start when the playhead is at
 * the video's end (after playing it through), so the music still has room.
 */
export function musicStartUs(project: Project, atUs: number): number {
  const end = videoDurationUs(project.timeline);
  return end > 0 && atUs >= end - 1_000_000 ? 0 : Math.max(0, atUs);
}

/** `source` as background music from `atUs` (the playhead when left out) to the video's end, ducking under speech. */
export function addMusic(source: MediaSource, atUs?: number): Promise<boolean> {
  const project = useProject.getState().snapshot?.project;
  if (!project) return Promise.resolve(false);
  const start = musicStartUs(project, atUs ?? usePlayback.getState().playheadUs);
  return placing(() => library.addMusic(source, start));
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

/** Where a drop at `target` starts: the time under the pointer, or a cut's time on the main track. */
function dropUs(project: Project, target: DropTarget): number {
  if (target.lane !== 'main') return target.atUs;
  const starts = clipStarts(project.timeline.main);
  return target.index < starts.length ? starts[target.index] : mainDurationUs(project.timeline.main);
}

/**
 * A pack item let go on the timeline: music where music goes (the Music lane), else a sound effect; a sticker as a
 * picture.
 */
export function dropPackItem(item: PackItem, target: DropTarget): Promise<boolean> {
  const project = useProject.getState().snapshot?.project;
  if (!project) return Promise.resolve(false);
  const source: MediaSource = { type: 'pack', id: item.id };
  const at = dropUs(project, target);
  if (item.category === 'sticker') return addImage(source, at);
  if (target.lane === 'sound' && target.row === 'music') return addMusic(source, at);
  return addSound(source, at);
}

/** A pack item at the playhead: music under the video, a sound effect, or a sticker. */
export function addPackItem(item: PackItem): Promise<boolean> {
  const source: MediaSource = { type: 'pack', id: item.id };
  if (item.category === 'music') return addMusic(source);
  if (item.category === 'sound') return addSound(source);
  return addImage(source);
}
