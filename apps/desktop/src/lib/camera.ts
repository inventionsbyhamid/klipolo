import type { Clip } from '../bindings/Clip';
import type { Project } from '../bindings/Project';
import type { Recording } from '../bindings/Recording';
import { cameraSource, firstAsset, screenSource } from './recording';
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
