import type { ArmOptions } from '../../bindings/ArmOptions';
import type { RecorderSetup } from '../../bindings/RecorderSetup';
import type { WhatToRecord } from '../../bindings/WhatToRecord';

/**
 * What the person chose in the Record dialog. Ids are the ones the setup lists (`RecorderSetup.devices`); a null mic or
 * camera is "No microphone" or "No camera".
 */
export interface Picks {
  /** "Whole screen", "One app", or "Record yourself" (the camera alone, with the mic: no screen). */
  kind: 'screen' | 'app' | 'camera';
  /** The display recorded, or the one the app's windows are recorded on; null when the setup lists none. */
  display: string | null;
  /** The app recorded (`AppInfo.appId`); null until one is chosen. */
  appId: string | null;
  mic: string | null;
  camera: string | null;
  /** "Show me on screen while recording": the camera's live bubble over the screen, never in the video. */
  showCamera: boolean;
  /** "Mirror my camera": the bubble and the take's camera layer show the camera as in a mirror. */
  mirror: boolean;
}

/** The choices before the setup has been read: nothing to record yet. */
export const NO_PICKS: Picks = {
  kind: 'screen',
  display: null,
  appId: null,
  mic: null,
  camera: null,
  showCamera: true,
  mirror: true,
};

/**
 * The choices the dialog opens with: the whole of the first display, the mic the app picked, and no camera. A picked
 * mic that isn't listed (gone as the setup was read) gives way to the first one listed, so the Microphone select
 * always names the mic a take records.
 */
export function defaultPicks(setup: RecorderSetup): Picks {
  const { displays, mics } = setup.devices;
  const listed = setup.mic === null || mics.some((m) => m.id === setup.mic);
  return { ...NO_PICKS, display: displays[0]?.id ?? null, mic: listed ? setup.mic : (mics[0]?.id ?? null) };
}

/**
 * The choices once "What to record" is `kind`. "Record yourself" with "No camera" chosen takes the camera the setup
 * offers (the person's last, else the first listed), as it records nothing without one.
 */
export function kindPicks(setup: RecorderSetup, picks: Picks, kind: Picks['kind']): Partial<Picks> {
  if (kind !== 'camera' || picks.camera !== null) return { kind };
  const { cameras } = setup.devices;
  const offered = cameras.some((c) => c.id === setup.camera) ? setup.camera : (cameras[0]?.id ?? null);
  return { kind, camera: offered };
}

/**
 * What a take of these choices records; null while there is no display, no app chosen for "One app", or no camera for
 * "Record yourself".
 */
export function whatToRecord(picks: Picks): WhatToRecord | null {
  if (picks.kind === 'camera') return picks.camera === null ? null : { kind: 'camera' };
  if (picks.display === null) return null;
  if (picks.kind === 'screen') return { kind: 'screen', display: picks.display };
  return picks.appId === null ? null : { kind: 'app', appId: picks.appId, display: picks.display };
}

/** The take as arming takes it; null when there is nothing to record yet (`whatToRecord`). */
export function armOptions(picks: Picks): ArmOptions | null {
  const what = whatToRecord(picks);
  if (what === null) return null;
  const camera = picks.camera;
  return {
    what,
    mic: picks.mic,
    camera,
    // Both are the camera's: without one there is nothing to show or mirror. The bubble shows over a recorded screen,
    // so the camera alone has none.
    cameraBubble: camera !== null && picks.showCamera && picks.kind !== 'camera',
    mirrorCamera: camera !== null && picks.mirror,
  };
}
