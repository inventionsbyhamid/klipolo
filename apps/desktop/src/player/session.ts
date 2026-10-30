import { useSyncExternalStore } from 'react';
import {
  engineAttach,
  enginePause,
  enginePlay,
  engineSeek,
  engineSetVisible,
  projectSummary,
  type EngineEvent,
  type EngineInfo,
  type FormatKind,
  type Presenter,
  type ProjectSummary,
} from '../bridge/engine';

/** starting: attaching; live: a frame is on screen; unavailable: the preview can't show anything. */
export type Status = 'starting' | 'live' | 'unavailable';

export interface Session {
  info: EngineInfo | null;
  presenter: Presenter;
  status: Status;
  /** A short note under the preview (a warning, or why it is unavailable). */
  message: string | null;
  /** Frames of other sessions belong to an earlier opening and are dropped. */
  session: number;
  project: ProjectSummary | null;
  format: FormatKind;
  playing: boolean;
  /** The playhead: `anchorUs` at `anchorMs` (ms since the epoch), moving at `rate` while playing. */
  anchorUs: number;
  anchorMs: number;
  rate: number;
  /** The newest seek's number: frames and `shown` events carrying an older one are stale. */
  seq: number;
}

const initial: Session = {
  info: null,
  presenter: 'socket',
  status: 'starting',
  message: null,
  session: 0,
  project: null,
  format: 'landscape',
  playing: false,
  anchorUs: 0,
  anchorMs: 0,
  rate: 1,
  seq: 0,
};

let state: Session = initial;
const listeners = new Set<() => void>();

function set(patch: Partial<Session>) {
  state = { ...state, ...patch };
  listeners.forEach((l) => l());
}

export function getSession(): Session {
  return state;
}

export function useSession<T>(select: (s: Session) => T): T {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => select(state),
  );
}

/** The project's length in µs (0 with none open). */
export function durationUs(s: Session = state): number {
  return s.project?.durationUs ?? 0;
}

/** Where the playhead is at `nowMs`: it runs on its own between the engine's reports while playing. */
export function playheadAt(nowMs: number, s: Session = state): number {
  if (!s.playing) return s.anchorUs;
  const at = s.anchorUs + (nowMs - s.anchorMs) * 1000 * s.rate;
  return Math.min(Math.max(at, 0), durationUs(s));
}

/** Whether a frame (or `shown` report) belongs to the newest seek. */
function current(seq: number): boolean {
  return ((seq - state.seq) | 0) >= 0;
}

function onEvent(event: EngineEvent) {
  switch (event.type) {
    case 'ready':
      set({ session: event.session });
      break;
    case 'state':
      set({ playing: event.playing, anchorUs: event.atUs, anchorMs: Date.now(), rate: event.rate });
      break;
    case 'shown':
      if (event.session !== state.session || !current(event.seq)) break;
      set({
        anchorUs: event.atUs,
        anchorMs: event.playback ? event.shownMs : Date.now(),
        status: 'live',
      });
      break;
    case 'presenter':
      set({ presenter: event.presenter, status: 'starting' });
      break;
    case 'warning':
      set({ message: event.message });
      break;
    case 'error':
      set({ status: 'unavailable', message: event.message });
      break;
    default:
      break;
  }
}

/** A frame the socket canvas drew: the playhead follows it. */
export function frameDrawn(timeUs: number, seq: number) {
  if (!current(seq)) return;
  set({ anchorUs: timeUs, anchorMs: Date.now(), status: 'live' });
}

/** The socket gave up reconnecting. */
export function socketFailed() {
  set({ status: 'unavailable', message: "The preview isn't available right now." });
}

/** The window was hidden or shown (minimised, the app hidden, another space): the engine pauses and lets its
 * decoders go while hidden. */
function onVisibility() {
  void engineSetVisible(document.visibilityState === 'visible').catch(() => undefined);
}

/** Attaches to the engine (once per page load), reads the open project and follows the window's visibility. */
export async function startSession(): Promise<void> {
  try {
    const project = await projectSummary();
    set({ project });
    const info = await engineAttach(onEvent);
    set({ info, presenter: info.presenter });
    // The engine listens once attached.
    onVisibility();
    document.addEventListener('visibilitychange', onVisibility);
    // Shows the first frame where the transport is.
    await seek(state.anchorUs);
  } catch (error) {
    set({ status: 'unavailable', message: String(error) });
  }
}

export function setFormat(format: FormatKind) {
  set({ format });
}

export async function seek(atUs: number): Promise<void> {
  const clamped = Math.min(Math.max(Math.round(atUs), 0), durationUs());
  const seq = (state.seq + 1) | 0;
  set({ seq, anchorUs: clamped, anchorMs: Date.now() });
  await engineSeek(clamped, seq);
}

export async function play(): Promise<void> {
  try {
    const from = Math.round(playheadAt(Date.now()));
    await enginePlay(from, 1, state.seq);
    set({ playing: true, anchorUs: from, anchorMs: Date.now(), message: null });
  } catch (error) {
    set({ message: String(error) });
  }
}

export async function pause(): Promise<void> {
  try {
    const at = await enginePause();
    set({ playing: false, anchorUs: at, anchorMs: Date.now() });
  } catch (error) {
    set({ message: String(error) });
  }
}

export function togglePlay(): Promise<void> {
  return state.playing ? pause() : play();
}

/** Moves the playhead by `deltaUs` from where it is now. */
export function skip(deltaUs: number): Promise<void> {
  return seek(playheadAt(Date.now()) + deltaUs);
}
