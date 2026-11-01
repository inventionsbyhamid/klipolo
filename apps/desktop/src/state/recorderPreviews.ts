import { create } from 'zustand';
import type { PreviewFrame } from '../bindings/PreviewFrame';
import type { PreviewId } from '../bindings/PreviewId';
import type { Previews } from '../bindings/Previews';
import type { RecorderEvent } from '../bindings/RecorderEvent';
import type { RecorderSetup } from '../bindings/RecorderSetup';
import type { WhatToRecord } from '../bindings/WhatToRecord';
import { isCode, saidOf } from '../bridge/call';
import { recorder } from '../bridge/recorder';
import { type Picks, whatToRecord } from '../lib/recorder/choices';
import { previewFrame, previewKind } from '../lib/recorder/previewWire';

/** A device whose preview waits for the person's Allow press the first time: macOS asks at that press. */
export type AskedDevice = 'camera' | 'mic';

/**
 * The Record dialog's previews as the page shows them: a still on each card, the live picture of what is
 * chosen, the camera's picture for its bubble, the mic's level. Pictures are object URLs of the frames' JPEGs, each let
 * go once a newer one replaces it or the previews stop.
 */
export interface RecorderPreviews {
  /** Each card's newest still, by its key (`display:<id>`, `app:<appId>`). */
  thumbs: Record<string, string>;
  /** When each card's newest still came (its frame's `atMs`): one a few seconds old is shown grey. */
  thumbsAt: Record<string, number>;
  /** The newest live picture of the screen or app chosen; null while there is none. */
  live: string | null;
  liveAt: number | null;
  /** The chosen camera's newest picture, for its bubble; null while there is none. */
  camera: string | null;
  cameraAt: number | null;
  /** The chosen mic's newest peak in dBFS; null until one comes (and once its preview stops). */
  meterDbfs: number | null;
  /**
   * The mic whose level can't come in this opening, by id: its preview stopped by itself (its `Failed`), which the app
   * doesn't start again until the dialog reads its setup again, or the app refused the previews. No level of it shows.
   */
  micStopped: string | null;
  /** "Allow camera" or "Allow microphone" was pressed in this opening: it isn't offered again. */
  asked: Record<AskedDevice, boolean>;
  /** macOS said no to the camera or the mic in the question an Allow press raised (its preview's `Failed`). */
  refused: Record<AskedDevice, boolean>;
  /**
   * The system refused the screen's pictures (`capture.permission.screen`): the cards keep their icons, and they
   * aren't asked for again in this opening.
   */
  screenRefused: boolean;
}

const NO_PICTURES = {
  thumbs: {},
  thumbsAt: {},
  live: null,
  liveAt: null,
  camera: null,
  cameraAt: null,
  meterDbfs: null,
} satisfies Partial<RecorderPreviews>;

const FRESH: RecorderPreviews = {
  ...NO_PICTURES,
  micStopped: null,
  asked: { camera: false, mic: false },
  refused: { camera: false, mic: false },
  screenRefused: false,
};

export const useRecorderPreviews = create<RecorderPreviews>(() => FRESH);

const set = useRecorderPreviews.setState;
const get = useRecorderPreviews.getState;

/** How long after the app answered busy (a take let go, whose worker hasn't gone yet) the previews are asked again. */
export const BUSY_RETRY_MS = 1_000;

/** The previews show: started, and neither stopped nor paused since. Frames and levels are drawn only then. */
let running = false;
/**
 * The app was asked for previews and not told to stop since: it may still run one (while Klipolo is behind, a camera
 * or mic preview waiting for macOS's answer), so a refusal of the camera or the mic is still heard.
 */
let started = false;
/** Openings so far: a refusal answered for an earlier opening is never put on a later one. */
let openings = 0;
/** What the latest start asked for. */
let wanted: Previews | null = null;
/** Starts so far: an answer is acted on only while its start is the latest. */
let starts = 0;
/** The app's calls go one at a time, in order: an off never overtakes the start before it. */
let chain: Promise<unknown> = Promise.resolve();
let busyTimer: ReturnType<typeof setTimeout> | undefined;

function inTurn(run: () => Promise<void>): Promise<void> {
  const next = chain.then(run, run);
  chain = next.catch(() => undefined);
  return next;
}

/** The card key a live picture of `what` carries (`display:<id>`, `app:<appId>`). */
export function cardKey(what: WhatToRecord | null): string | null {
  if (what === null) return null;
  return what.kind === 'screen' ? `display:${what.display}` : `app:${what.appId}`;
}

/**
 * What the dialog previews for these choices: every card's still and the live picture of what is chosen while Screen
 * Recording is allowed (`screenRefused`: a take or the previews were refused it in this opening), and the camera and
 * the mic chosen (the app starts those only once allowed, or on an Allow press). Null when nothing should run: no
 * setup yet, or a take runs.
 */
export function previewsFor(setup: RecorderSetup | null, picks: Picks, screenRefused: boolean): Previews | null {
  if (setup === null || setup.busy || setup.ownTake) return null;
  const screen = setup.permissions.screen === 'granted' && !screenRefused;
  const cards = setup.devices.displays.length + setup.devices.apps.length;
  return {
    thumbs: screen && cards > 0,
    live: screen ? whatToRecord(picks) : null,
    livePixels: null,
    camera: picks.camera,
    mic: picks.mic,
    ask: null,
  };
}

function letGo(...urls: (string | null | undefined)[]) {
  for (const url of urls) if (url) URL.revokeObjectURL(url);
}

/** The pictures and level a start no longer wants go now, so none of an earlier choice is shown as this one's. */
function dropUnwanted(next: Previews) {
  const s = get();
  const before = wanted;
  const drop: Partial<RecorderPreviews> = {};
  if (!next.thumbs) {
    letGo(...Object.values(s.thumbs));
    Object.assign(drop, { thumbs: {}, thumbsAt: {} });
  }
  if (cardKey(next.live) !== cardKey(before?.live ?? null)) {
    letGo(s.live);
    Object.assign(drop, { live: null, liveAt: null });
  }
  if (next.camera !== before?.camera) {
    letGo(s.camera);
    Object.assign(drop, { camera: null, cameraAt: null });
  }
  if (next.mic !== before?.mic) drop.meterDbfs = null;
  if (Object.keys(drop).length > 0) set(drop);
}

/** Every picture and the level go (the previews stopped). */
function dropAll() {
  const s = get();
  letGo(...Object.values(s.thumbs), s.live, s.camera);
  set(NO_PICTURES);
}

/**
 * One frame: drawn when it is of what is wanted now, else dropped. Either way it is acked once, so the app goes on;
 * one that doesn't read is acked by the kind its header names, if it names one. A frame that can't be drawn is
 * skipped, never thrown back into the channel, which would hold every frame after it.
 */
function onFrame(message: ArrayBuffer) {
  const read = previewFrame(message);
  try {
    if (read !== null && running && wanted !== null) draw(read.frame, read.jpeg, wanted);
  } catch {
    // Skipped: the last picture stays, and the next frame is drawn.
  }
  const kind = read?.frame.kind ?? previewKind(message);
  if (kind !== null) void recorder.previewAck(kind).catch(() => undefined);
}

function draw({ kind, key, atMs }: PreviewFrame, jpeg: Blob, want: Previews) {
  const s = get();
  if (kind === 'thumb') {
    if (!want.thumbs) return;
    const url = URL.createObjectURL(jpeg);
    set({ thumbs: { ...s.thumbs, [key]: url }, thumbsAt: { ...s.thumbsAt, [key]: atMs } });
    letGo(s.thumbs[key]);
  } else if (kind === 'live') {
    // A picture of an earlier choice, still on its way, is never shown as this one's.
    if (key !== cardKey(want.live)) return;
    set({ live: URL.createObjectURL(jpeg), liveAt: atMs });
    letGo(s.live);
  } else {
    if (key !== want.camera) return;
    set({ camera: URL.createObjectURL(jpeg), cameraAt: atMs });
    letGo(s.camera);
  }
}

/** The device whose refusal `code` is, if it is a camera's or a mic's. */
function refusedDevice(code: string): AskedDevice | null {
  if (code === 'capture.permission.camera') return 'camera';
  if (code === 'capture.permission.mic') return 'mic';
  return null;
}

/** macOS refuses `device`: its row says so, with its privacy page, and its picture or level goes. */
function deviceRefused(device: AskedDevice) {
  const s = get();
  if (device === 'camera') letGo(s.camera);
  set({
    refused: { ...s.refused, [device]: true },
    ...(device === 'camera' ? { camera: null, cameraAt: null } : { meterDbfs: null }),
  });
}

/**
 * The app refused a start: the camera or the mic (an Allow press for a device macOS refuses; said in its row however
 * late, within this opening), the screen's pictures (the cards keep their icons), busy (asked again soon), or nothing.
 */
function refusedStart(mine: number, opening: number, want: Previews, error: unknown) {
  const said = saidOf(error);
  const device = typeof said === 'string' ? null : refusedDevice(said.code);
  if (device !== null) {
    if (opening === openings) deviceRefused(device);
    return;
  }
  if (mine !== starts || !running) return;
  if (isCode(error, 'capture.permission.screen')) {
    screenPicturesRefused();
  } else if (isCode(error, 'capture.busy') && wanted !== null) {
    // A take let go still holds the app until its worker has gone: asked again once it may have.
    const again = wanted;
    clearTimeout(busyTimer);
    busyTimer = setTimeout(() => {
      if (mine === starts && running) startPreviews(again);
    }, BUSY_RETRY_MS);
  } else {
    // Anything else (`capture.error.testRun` in a test run): nothing shows, not even an empty level, and nothing is
    // asked again.
    set({ micStopped: want.mic });
  }
}

function screenPicturesRefused() {
  const s = get();
  letGo(...Object.values(s.thumbs), s.live);
  set({ thumbs: {}, thumbsAt: {}, live: null, liveAt: null, screenRefused: true });
}

/**
 * The previews should show `want` (on opening, on every change of choice, and back in front): the app is told, and the
 * pictures a change leaves behind go at once.
 */
export function startPreviews(want: Previews): void {
  const mine = ++starts;
  const opening = openings;
  clearTimeout(busyTimer);
  dropUnwanted(want);
  running = true;
  started = true;
  wanted = want;
  void inTurn(() => recorder.previews(want, onFrame)).catch((error: unknown) =>
    refusedStart(mine, opening, want, error),
  );
}

/** "Allow camera" or "Allow microphone": the previews as they are, asking for `device`, which is when macOS asks. */
export function allowPreview(device: AskedDevice): void {
  set({ asked: { ...get().asked, [device]: true } });
  if (wanted !== null) startPreviews({ ...wanted, ask: device });
}

/**
 * Klipolo went behind (another app has the focus): the page shows no preview until it is back in front (then
 * `startPreviews`). The app stops them itself, but for one waiting for macOS's answer to an Allow press, so nothing is
 * sent: an off would close that one too, and the answer would be lost. A refusal of the camera or the mic is still
 * heard.
 */
export function pausePreviews(): void {
  clearTimeout(busyTimer);
  if (!running) return;
  starts++;
  running = false;
  wanted = null;
  dropAll();
}

/**
 * Every preview stops (Record or Try again, the page hidden, the dialog closing or going), and its pictures and level
 * go. Nothing is sent when the app runs none.
 */
export function stopPreviews(): void {
  pausePreviews();
  if (!started) return;
  started = false;
  void inTurn(() => recorder.previewsOff()).catch(() => undefined);
}

/** The dialog closed: the previews stop, and what this opening pressed and was refused is forgotten. */
export function forgetPreviews(): void {
  stopPreviews();
  openings++;
  set(FRESH);
}

const PREVIEW_IDS: ReadonlySet<string> = new Set<PreviewId>([
  'preview-thumbs',
  'preview-live',
  'preview-camera',
  'preview-mic',
]);

/**
 * A `recorder-event` of a preview's (its id is a `PreviewId`, never a take's): the mic's level, or why a preview
 * stopped by itself, whose picture or level then goes so nothing stale looks live. True when it was a preview's.
 */
export function hearPreview(e: RecorderEvent): boolean {
  if (!PREVIEW_IDS.has(e.take)) return false;
  const id = e.take as PreviewId;
  const ev = e.event;
  if (!running) {
    // Behind, macOS's answer to an Allow press may still come: a no is said in its row.
    const device = ev.ev === 'failed' ? refusedDevice(ev.error.code) : null;
    if (started && device !== null && id === `preview-${device}`) deviceRefused(device);
    return true;
  }
  if (ev.ev === 'meter') {
    if (id === 'preview-mic' && ev.source === 'mic' && wanted !== null && wanted.mic !== null)
      set({ meterDbfs: ev.peakDbfs });
  } else if (ev.ev === 'failed') {
    stopped(id, ev.error.code);
  }
  return true;
}

function stopped(id: PreviewId, code: string) {
  const s = get();
  switch (id) {
    case 'preview-thumbs':
    case 'preview-live':
      if (code === 'capture.permission.screen') return screenPicturesRefused();
      if (id === 'preview-thumbs') {
        letGo(...Object.values(s.thumbs));
        return set({ thumbs: {}, thumbsAt: {} });
      }
      letGo(s.live);
      return set({ live: null, liveAt: null });
    case 'preview-camera':
      if (code === 'capture.permission.camera') return deviceRefused('camera');
      letGo(s.camera);
      return set({ camera: null, cameraAt: null });
    case 'preview-mic':
      if (code === 'capture.permission.mic') return deviceRefused('mic');
      return set({ meterDbfs: null, micStopped: wanted?.mic ?? null });
  }
}

/** Back to as at launch, without a word to the app (tests). */
export function resetPreviews(): void {
  clearTimeout(busyTimer);
  running = false;
  started = false;
  wanted = null;
  starts++;
  openings++;
  chain = Promise.resolve();
  dropAll();
  set(FRESH);
}
