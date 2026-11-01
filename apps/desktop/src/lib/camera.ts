import type { CameraLayout } from '../bindings/CameraLayout';
import type { CameraMode } from '../bindings/CameraMode';
import type { Clip } from '../bindings/Clip';
import type { FormatKind } from '../bindings/FormatKind';
import type { Item } from '../bindings/Item';
import type { Project } from '../bindings/Project';
import type { Recording } from '../bindings/Recording';
import { cameraAssets, cameraSource, firstAsset, primarySource, screenSource } from './recording';
import { assetById } from './timeline';

/** A recording clip's camera: where the engine draws it, and what the canvas and the clip card offer for it. */

/** The recording `clip` plays, if it plays one. */
function recordingOf(project: Project, clip: Clip): Recording | undefined {
  const source = clip.source;
  return source.type === 'recording' ? project.recordings.find((r) => r.id === source.id) : undefined;
}

/**
 * Whether `clip` draws its recording's camera beside the screen, as a bubble or a split (as the engine draws it): a
 * clip with a camera layout whose recording has a screen and a camera, both with video.
 */
export function hasCameraOverlay(project: Project, clip: Clip): boolean {
  const recording = clip.camera ? recordingOf(project, clip) : undefined;
  const video = (asset: string | null) => assetById(project, asset)?.kind === 'video';
  return !!recording && video(firstAsset(screenSource(recording))) && video(firstAsset(cameraSource(recording)));
}

/** Whether `clip` plays a take of the camera alone: the camera is its main footage, which its camera layout mirrors. */
export function isCameraOnly(project: Project, clip: Clip): boolean {
  const recording = clip.camera ? recordingOf(project, clip) : undefined;
  return !!recording && primarySource(recording)?.role === 'camera';
}

/** The files of `clip`'s recording camera (one per run; usually one), if it has one. */
export function recordingCameras(project: Project, clip: Clip): string[] {
  const recording = recordingOf(project, clip);
  return recording ? cameraAssets(recording) : [];
}

/** The (first) layer `clip`'s camera was separated into, or null. */
export function separatedCamera(project: Project, clip: Clip): Item | null {
  const cameras = recordingCameras(project, clip);
  if (cameras.length === 0) return null;
  for (const track of project.timeline.tracks) {
    for (const item of track.items) {
      const content = item.content;
      if (content.type === 'video' && cameras.includes(content.assetId) && item.attach?.clipId === clip.id)
        return item;
    }
  }
  return null;
}

/**
 * The camera layout in `format` only: that format's own mode, never the base one, so the other formats keep theirs
 * (and Split, which suits 9:16, never becomes every format's layout). `camera` is the clip's camera as the project has
 * it when the change is built.
 */
export function cameraModePatch(camera: CameraLayout, mode: CameraMode, format: FormatKind): Partial<CameraLayout> {
  return { modeOverrides: { ...camera.modeOverrides, [format]: mode } };
}
