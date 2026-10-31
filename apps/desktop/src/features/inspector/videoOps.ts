import type { Clip } from '../../bindings/Clip';
import type { Item } from '../../bindings/Item';
import type { Project } from '../../bindings/Project';
import type { Label } from '../../lib/labels';
import { cameraAssets } from '../../lib/recording';
import { findClip } from '../../lib/timeline';

/** The speeds a video layer offers; its length follows (the model rescales it onto the same footage). */
const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 2];

/**
 * The speeds to choose from for a layer playing at `current`: `SPEEDS`, and `current` when it is another (set from
 * chat), so the select always shows what is set.
 */
export function speedChoices(current: number): number[] {
  return SPEEDS.includes(current) ? SPEEDS : [...SPEEDS, current].sort((a, b) => a - b);
}

/** The files of `clip`'s recording camera (one per run; usually one), if it has one. */
function recordingCameras(project: Project, clip: Clip): string[] {
  if (clip.source.type !== 'recording') return [];
  const id = clip.source.id;
  const recording = project.recordings.find((r) => r.id === id);
  return recording ? cameraAssets(recording) : [];
}

/**
 * The clip whose camera this video layer is (a separated camera, attached to its clip), or null. It plays at its clip's
 * speed so the lips stay in sync with the mic: its Speed is the clip's.
 */
export function cameraClipOf(project: Project, item: Item): Clip | null {
  if (item.content.type !== 'video' || !item.attach) return null;
  const clip = findClip(project, item.attach.clipId);
  return clip && recordingCameras(project, clip).includes(item.content.assetId) ? clip : null;
}

/** Mirror's undo label for turning it `on` or off: a separated camera's is said as the camera's. */
export function mirrorLabel(camera: boolean, on: boolean): Label {
  if (camera) return on ? 'camera.mirror.undo' : 'camera.unmirror.undo';
  return on ? 'media.mirror.undo' : 'media.unmirror.undo';
}
