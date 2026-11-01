import type { CameraLayout } from '../bindings/CameraLayout';
import type { CameraMode } from '../bindings/CameraMode';
import type { Clip } from '../bindings/Clip';
import type { FormatKind } from '../bindings/FormatKind';
import type { Item } from '../bindings/Item';
import type { Project } from '../bindings/Project';
import type { Recording } from '../bindings/Recording';
import { cameraAssets, cameraSource, firstAsset, primarySource, screenSource } from './recording';
import { assetById, clipDurationUs, itemEndUs } from './timeline';

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

/** Where `clip` plays on the main track, in µs, or null when it isn't there. O(clips). */
function spanOf(project: Project, clip: Clip): [number, number] | null {
  let start = 0;
  for (const c of project.timeline.main) {
    const end = start + clipDurationUs(c);
    if (c.id === clip.id) return [start, end];
    start = end;
  }
  return null;
}

/**
 * The layer that plays `clip`'s recording camera over the clip: one of its files, on any track, kept with the clip or
 * freed from it, meeting the clip's time. The camera is its own layer then, as Rust's `DetachSource` decides when it
 * refuses to separate it again; showing the clip's own camera as well would draw it twice. Null when none does.
 * O(clips + layers).
 */
export function cameraLayer(project: Project, clip: Clip): Item | null {
  const cameras = recordingCameras(project, clip);
  const span = cameras.length > 0 ? spanOf(project, clip) : null;
  if (!span) return null;
  const [start, end] = span;
  for (const track of project.timeline.tracks) {
    for (const item of track.items) {
      const content = item.content;
      if (
        (content.type === 'video' || content.type === 'audio') &&
        cameras.includes(content.assetId) &&
        item.startUs < end &&
        itemEndUs(item) > start
      )
        return item;
    }
  }
  return null;
}

/**
 * `clip`'s camera, for Separate camera and the cards: none (no camera layout or no recording camera beside the
 * footage), separable, or separated (`cameraLayer`). One answer for the command, the menu and both cards.
 */
export function cameraState(project: Project, clip: Clip): 'none' | 'separable' | 'separated' {
  if (!clip.camera || recordingCameras(project, clip).length === 0) return 'none';
  return cameraLayer(project, clip) ? 'separated' : 'separable';
}

/**
 * The camera layout in `format` only: that format's own mode, never the base one, so the other formats keep theirs
 * (and Split, which suits 9:16, never becomes every format's layout). `camera` is the clip's camera as the project has
 * it when the change is built.
 */
export function cameraModePatch(camera: CameraLayout, mode: CameraMode, format: FormatKind): Partial<CameraLayout> {
  return { modeOverrides: { ...camera.modeOverrides, [format]: mode } };
}
