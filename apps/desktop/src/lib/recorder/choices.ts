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
  /**
   * The camera each mode had when the person last left it (`kindPicks`): a screen take's (Whole screen and One app) and
   * Record yourself's, so going back to one brings back its own camera, never the other's.
   */
  cameras: Record<CameraMode, string | null>;
}

/** Whose camera a kind of take keeps: a screen take's bubble, or Record yourself's picture. */
type CameraMode = 'screen' | 'yourself';

const modeOf = (kind: Picks['kind']): CameraMode => (kind === 'camera' ? 'yourself' : 'screen');

/** The choices before the setup has been read: nothing to record yet. */
export const NO_PICKS: Picks = {
  kind: 'screen',
  display: null,
  appId: null,
  mic: null,
  camera: null,
  showCamera: true,
  mirror: true,
  cameras: { screen: null, yourself: null },
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
 * The choices once "What to record" is `kind`. Each mode keeps its own camera: going between a screen take (Whole
 * screen, One app) and Record yourself puts away the camera of the mode left and brings back the one the other had, so
 * no camera chosen for one turns up as a bubble in the other. Record yourself with no camera of its own takes the
 * camera the setup offers (the person's last, else the first listed), as it records nothing without one.
 */
export function kindPicks(setup: RecorderSetup, picks: Picks, kind: Picks['kind']): Partial<Picks> {
  const [from, to] = [modeOf(picks.kind), modeOf(kind)];
  if (from === to) return { kind };
  const cameras = { ...picks.cameras, [from]: picks.camera };
  const { cameras: listed } = setup.devices;
  const offered = listed.some((c) => c.id === setup.camera) ? setup.camera : (listed[0]?.id ?? null);
  const camera = cameras[to] ?? (to === 'yourself' ? offered : null);
  return { kind, camera, cameras };
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
