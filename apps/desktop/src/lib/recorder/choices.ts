import type { ArmOptions } from '../../bindings/ArmOptions';
import type { RecorderSetup } from '../../bindings/RecorderSetup';
import type { WhatToRecord } from '../../bindings/WhatToRecord';

/**
 * What the person chose in the Record dialog. Ids are the ones the setup lists (`RecorderSetup.devices`); a null mic or
 * camera is "No microphone" or "No camera".
 */
export interface Picks {
  /** "Whole screen" or "One app". */
  kind: 'screen' | 'app';
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

/** The choices the dialog opens with: the whole of the first display, the mic the app picked, and no camera. */
export function defaultPicks(setup: RecorderSetup): Picks {
  return { ...NO_PICKS, display: setup.devices.displays[0]?.id ?? null, mic: setup.mic };
}

/** What a take of these choices records; null while there is no display, or no app chosen for "One app". */
export function whatToRecord(picks: Picks): WhatToRecord | null {
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
    // Both are the camera's: without one there is nothing to show or mirror.
    cameraBubble: camera !== null && picks.showCamera,
    mirrorCamera: camera !== null && picks.mirror,
  };
}
