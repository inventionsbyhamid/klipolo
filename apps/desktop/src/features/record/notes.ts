import type { CodedError } from '../../bindings/CodedError';
import type { RecorderSetup } from '../../bindings/RecorderSetup';
import { codedMessage } from '../../bridge/call';
import type { Picks } from '../../lib/recorder/choices';
import { screenStep } from '../../lib/recorder/permission';

const coded = (code: string, params: CodedError['params'] = {}): CodedError => ({ code, params });

/**
 * The setup's notes that stop a take from starting, first first: the app refuses to arm with them, so Record is off
 * and says the first one.
 */
const BLOCKING = [
  'capture.preflight.osTooOld',
  'capture.preflight.diskBlock',
  'capture.preflight.networkFolder',
  'capture.preflight.noEncoder',
];

/**
 * Notes the setup words about the default camera and mic: the dialog says them for the devices chosen instead
 * (`notesBefore`), and the camera's fit for the take's display under the camera (`cameraNote`).
 */
const FOR_THE_CHOICE: ReadonlySet<string> = new Set([
  'capture.preflight.cameraBusy',
  'capture.preflight.cameraOff',
  'capture.preflight.virtualMic',
  'capture.preflight.cameraWithScreen',
]);

/** Notes about the mic the setup picked, said only while it is the one chosen. */
const FOR_THE_PICKED_MIC: ReadonlySet<string> = new Set([
  'capture.preflight.bluetoothMic',
  'capture.preflight.micMissing',
]);

/** "A recording is already running. Stop it from the menu bar.": where the recorder's own item is. */
export function busySentence(setup: RecorderSetup): string {
  const place = codedMessage(coded(`capture.place.${setup.caps.indicator}`));
  return codedMessage(coded('capture.busy', { place }));
}

/** What the dialog says before Record, in the setup's order, for the devices chosen. */
export function notesBefore(setup: RecorderSetup, picks: Picks): CodedError[] {
  const notes = setup.notes.filter(
    (n) => !FOR_THE_CHOICE.has(n.code) && (!FOR_THE_PICKED_MIC.has(n.code) || picks.mic === setup.mic),
  );
  const camera = setup.devices.cameras.find((c) => c.id === picks.camera);
  if (camera?.inUse) notes.push(coded('capture.preflight.cameraBusy', { device: camera.name }));
  else if (camera?.suspended) notes.push(coded('capture.preflight.cameraOff', { device: camera.name }));
  const mic = setup.devices.mics.find((m) => m.id === picks.mic);
  if (mic?.isVirtual) notes.push(coded('capture.preflight.virtualMic', { device: mic.name }));
  return notes;
}

/**
 * Why the chosen camera isn't recorded beside the screen at the quality the take records at, null when it is (or with
 * "No camera"). The quality is the one the app plans the take at: the best offer for a whole screen or an app's
 * windows on the take's display with the camera on (else its first), from the setup the dialog shows. Arming
 * starts the take at once, so the person reads this here, before Record.
 */
export function cameraNote(setup: RecorderSetup, picks: Picks): CodedError | null {
  if (picks.camera === null) return null;
  const listed = setup.displays.find((d) => d.display === picks.display) ?? setup.displays[0];
  const fit = listed ? (listed.withCamera ?? listed.noCamera) : { choices: setup.choices, offers: setup.offers };
  const offer = fit.offers.find((o) => o.bestNative) ?? fit.offers[0];
  return offer ? offer.cameraNote : fit.choices.cameraNote;
}

/** The setup lists no screen: there is nothing to choose, and a new reading may find one. */
export const NO_SCREEN = 'Klipolo found no screen to record. Close Record and open it again.';

/** Why Record can't start yet, as the dialog says it; null when it can. */
export type NotReady =
  /** A take runs: it stops from the recorder's own item. */
  | { why: 'busy'; text: string }
  /** The setup's own reason (too little space, no encoder). */
  | { why: 'note'; text: string }
  /** The Screen Recording step shows: a press on Record goes to its button. */
  | { why: 'screen'; text: string }
  | { why: 'choose'; text: string };

/**
 * Record's reason to wait, first first: a take already runs, the setup's own block, Screen Recording not allowed yet,
 * then nothing chosen to record. The store arms nothing in these cases either; this says why.
 */
export function notReady(setup: RecorderSetup, picks: Picks, screenRefused: boolean): NotReady | null {
  if (setup.busy || setup.ownTake) return { why: 'busy', text: busySentence(setup) };
  const block = setup.notes.find((n) => BLOCKING.includes(n.code));
  if (block) return { why: 'note', text: codedMessage(block) };
  if (!setup.caps.osSupported) return { why: 'note', text: codedMessage(coded('capture.error.unsupported')) };
  if (screenStep(setup, picks, screenRefused)) return { why: 'screen', text: 'Allow screen recording first' };
  if (picks.display === null) return { why: 'choose', text: NO_SCREEN };
  if (picks.kind === 'app' && picks.appId === null) return { why: 'choose', text: 'Choose an app first' };
  return null;
}

/** A take's source by its id ("camera", "mic", "camera-2"), as the recorder names it ("Camera"), else the id. */
export function sourceName(id: string): string {
  return codedMessage(coded(`capture.source.${id.replace(/[-_]\d+$/, '')}`)) || id;
}

/** "{source} isn't sending anything yet.", for the start gate. */
export function gateSentence(id: string): string {
  return codedMessage(coded('capture.gate.waiting', { source: sourceName(id) }));
}

/** The privacy page a take's refusal is about (the microphone or the camera), by its code; else null. */
export function refusedPage(code: string | null): 'mic' | 'camera' | null {
  if (code === 'capture.permission.mic') return 'mic';
  if (code === 'capture.permission.camera') return 'camera';
  return null;
}
