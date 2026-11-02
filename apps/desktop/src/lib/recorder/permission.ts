import type { RecorderSetup } from '../../bindings/RecorderSetup';
import type { Picks } from './choices';

/**
 * Which kinds of take record through macOS's Screen Recording: the whole screen and one app's windows both do; "Record
 * yourself" (the camera and the mic) never does.
 */
const RECORDS_THE_SCREEN: Record<Picks['kind'], boolean> = { screen: true, app: true, camera: false };

/**
 * Whether the dialog shows its Screen Recording step in place of Record: the take records the screen, and macOS hasn't
 * allowed Klipolo to yet, or a take was refused it as it started (`refused`). Record arms nothing while it shows, so
 * the only request for Screen Recording is the person's "Allow screen recording".
 */
export function screenStep(setup: RecorderSetup, picks: Picks, refused: boolean): boolean {
  return RECORDS_THE_SCREEN[picks.kind] && (setup.permissions.screen !== 'granted' || refused);
}

/**
 * The Screen Recording step's card: `ask` first; `reopen` once "Allow screen recording" was pressed in this run of
 * Klipolo (macOS applies the grant only to a Klipolo started after it); `stillRefused` when Klipolo reopened for it and
 * macOS still says no, which happens when an older copy of Klipolo is in the list. That card says how to remove it,
 * instead of asking in a loop.
 */
export type ScreenStage = 'ask' | 'reopen' | 'stillRefused';

/** Which card the Screen Recording step shows; `asked`: "Allow screen recording" was pressed in this run of Klipolo. */
export function screenStage(setup: RecorderSetup, asked: boolean): ScreenStage {
  if (asked) return 'reopen';
  return setup.screenRefusedAfterReopen ? 'stillRefused' : 'ask';
}

/**
 * Whether the dialog says the chosen mic or camera isn't allowed (with "Open privacy settings"): macOS denied it, and
 * it is chosen. With "No microphone" or "No camera" there is nothing to say, and Record still records.
 */
export function deviceDenied(setup: RecorderSetup, kind: 'mic' | 'camera', picks: Picks): boolean {
  return setup.permissions[kind] === 'denied' && picks[kind] !== null;
}
