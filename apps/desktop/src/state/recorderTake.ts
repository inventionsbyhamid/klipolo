import { t } from '@lingui/core/macro';
import type { StoreApi } from 'zustand';
import type { CaptureEvent } from '../bindings/CaptureEvent';
import type { CaptureLevel } from '../bindings/CaptureLevel';
import type { CodedError } from '../bindings/CodedError';
import type { RecorderEvent } from '../bindings/RecorderEvent';
import { codedMessage, isCode, saidOf } from '../bridge/call';
import { recorder } from '../bridge/recorder';
import { errorMessage } from '../lib/errors';
import { armOptions } from '../lib/recorder/choices';
import { screenStep } from '../lib/recorder/permission';
import { phaseOf } from '../lib/recorder/phase';
import type { RecorderState, RecorderStore } from './recorder';
import { stopPreviews } from './recorderPreviews';
import { toast } from './toasts';

type Set = StoreApi<RecorderStore>['setState'];
type Get = StoreApi<RecorderStore>['getState'];

/** The take's part of the state when there is none: the choices show. */
export const NO_TAKE = {
  take: null,
  worker: null,
  countdown: null,
  gate: null,
  meterDbfs: null,
} satisfies Partial<RecorderState>;

/** Where a refusal came from: a take (Record or Try again) or another press (`RecorderState.errorFrom`). */
export type ErrorFrom = 'take' | 'press';

/** No refusal to say. */
export const NO_ERROR = { error: null, errorCode: null, errorFrom: null } satisfies Partial<RecorderState>;

/** `error` (from `call`) as the store keeps it: its sentence, its code when it has one, and who was refused. */
export function refused(error: unknown, from: ErrorFrom): Pick<RecorderState, 'error' | 'errorCode' | 'errorFrom'> {
  const said = saidOf(error);
  return { error: errorMessage(error), errorCode: typeof said === 'string' ? null : said.code, errorFrom: from };
}

/**
 * How long Record waits for the last take's worker to go (its `state: idle`) before it arms the next: the app is busy
 * until then.
 */
export const LEAVE_WAIT_MS = 5_000;

/** Events kept for a take whose id arming hasn't given yet: its first few. */
const HELD_KEPT = 32;
/** The app refuses a command for a take that has ended with this: nothing is left to say. */
const STALE = 'capture.error.staleTake';
/** A take refused Screen Recording as it started: the dialog shows its step again. */
const SCREEN_REFUSED = 'capture.permission.screen';
/** The source whose level the dialog's meter shows. */
const MIC = 'mic';
/** The toast that says what happened to a take while Klipolo's window was hidden: one at a time. */
const NOTE_TOAST = 'recorder-note';
/**
 * Notes kept to say once Klipolo is back, besides every critical one: the camera stopped while the screen kept
 * recording, which the window, hidden for the take, can't show when it happens.
 */
const KEPT_NOTES: ReadonlySet<string> = new Set(['capture.note.cameraDropped']);
/**
 * A take was added to its project, which then couldn't be opened to show it: news, not a failure ("The recording was
 * added to your project. Open the project from the start screen to see it.").
 */
const ADDED_NOT_OPENED = 'capture.import.addedNotOpened';
/** The toast that says what became of a saved take's landing when it didn't show in the editor. */
const LANDING_TOAST = 'recorder-landing';

/** A note of this kind is said as Klipolo comes back from a take, so a landed take's own notes leave it out. */
export function saidOnReturn(code: string): boolean {
  return KEPT_NOTES.has(code);
}

/** One Record press, from the press until its Start has been sent. */
interface Press {
  /** Cancel, Esc or closing the dialog came first: it never starts. */
  cancelled: boolean;
}

/** A take let go (cancelled, failed or saved), until its worker has gone and the app is free for the next one. */
interface Leaving {
  id: string;
  gone: () => void;
  ended: Promise<void>;
}

export interface TakeSlice {
  actions: Pick<RecorderStore, 'record' | 'cancel' | 'recordWithout'>;
  /** One `recorder-event`: the dialog's take's, or one it lets go; any other take's is ignored. */
  hear: (event: RecorderEvent) => void;
  /**
   * The dialog closes. Nothing records without it: a take on its way is cancelled, and a recording one is stopped and
   * saved, then followed to its end.
   */
  closing: () => void;
  /** Forgets every press, take and held event (tests). */
  forget: () => void;
}

/** A coded sentence from the recorder, in English. */
function said(error: CodedError): string {
  return codedMessage(error) || t({ id: 'error.unknown', message: 'Something went wrong. Try again.' });
}

/**
 * The take's part of the recorder store: Record arms the take, then sends its Start; the take's events move the
 * dialog through the countdown, the start gate, the recording and the save; Cancel lets it go. Only the dialog's take
 * is heard, by its id, so a late event from an earlier take never reaches a newer one. `went` hears that a take let
 * go has gone (its worker's `state: idle`), so the app is free for the next.
 */
export function takeSlice(set: Set, get: Get, went: () => void): TakeSlice {
  /** The press still arming or starting. */
  let press: Press | null = null;
  /** The take Start was sent to: Cancel ends it in its countdown, else Disarm lets it go before it ever started. */
  let started: string | null = null;
  let leaving: Leaving | null = null;
  /** What a take said before arming gave its id. */
  const held: RecorderEvent[] = [];
  /** The countdown when the start gate asked: a lower one means nothing holds it any more. */
  let gateFrom: number | null = null;
  /** The kept note is critical: only another critical one replaces it. */
  let noteCritical = false;
  /** The kept note says the take was added (`ADDED_NOT_OPENED`): said as news, not as a failure. */
  let noteAdded = false;
  /**
   * The take saved and let go whose landing is still to come (it lands before its worker's `state: idle`): what its
   * landing says, when it didn't land in the editor, is still heard.
   */
  let landing: string | null = null;
  /**
   * The take that passed the start gate (its worker said `recording` or `paused`): it is saved however it ends, so the
   * dialog goes with it. A `stopping` alone proves nothing: a take stopped in its countdown says it too.
   */
  let recorded: string | null = null;
  /** Record presses so far: a sentence about an earlier take is never put on a later press. */
  let presses = 0;
  /** Takes whose worker said it has gone (`state: idle`): nothing of them runs any more. */
  const departed = new Set<string>();

  function leave(id: string) {
    leaving?.gone();
    let gone!: () => void;
    const ended = new Promise<void>((resolve) => (gone = resolve));
    leaving = { id, gone, ended };
    set({ lastTake: landing === id ? 'adding' : 'stopping' });
  }

  function gone(id: string) {
    departed.add(id);
    if (landing === id) landing = null;
    if (leaving?.id !== id) return;
    leaving.gone();
    leaving = null;
    set({ lastTake: null });
    went();
  }

  /** The last take's worker has gone, or `LEAVE_WAIT_MS` passed (then the app says if it is still busy). */
  async function lastTakeGone() {
    const last = leaving;
    if (last === null) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const waited = new Promise<void>((resolve) => (timer = setTimeout(resolve, LEAVE_WAIT_MS)));
    await Promise.race([last.ended, waited]);
    clearTimeout(timer);
  }

  /**
   * Tells take `id` to end with `command`. A take that has gone already needs nothing more. Any other refusal falls back
   * to "Stop the running recording", which never starts capture; only if that fails too is it said, and never once a
   * later Record was pressed. Once a later take is the dialog's, the app runs that one (one take at a time), so `id` has
   * gone and nothing is stopped. Nor once `id`'s worker said it has gone: what runs then is a later take, perhaps
   * still arming.
   */
  async function tell(id: string, command: 'cancel' | 'disarm' | 'stop') {
    const mine = presses;
    try {
      await recorder.send(id, { cmd: command });
    } catch (error) {
      if (isCode(error, STALE)) return gone(id);
      const now = get().take;
      if ((now !== null && now !== id) || departed.has(id)) return;
      try {
        await recorder.stopRunning();
      } catch (stopError) {
        if (presses !== mine) return;
        if (get().open) set(refused(stopError, 'press'));
        else toast.error(errorMessage(stopError));
      }
    }
  }

  /** Lets take `id` go with `command`: only its last word is heard from now on. */
  async function letGo(id: string, command: 'cancel' | 'disarm') {
    leave(id);
    if (started === id) started = null;
    await tell(id, command);
  }

  /** What the take said while Klipolo's window was hidden, said now that it is back. */
  function sayNote() {
    const { note } = get();
    if (note === null) return;
    set({ note: null });
    noteCritical = false;
    if (noteAdded) toast.info(note, { key: NOTE_TOAST });
    else toast.error(note, { key: NOTE_TOAST });
  }

  function keepNote(note: CodedError, level: CaptureLevel) {
    const critical = level === 'critical';
    if (!critical && !KEPT_NOTES.has(note.code)) return;
    if (noteCritical && !critical) return;
    noteCritical = critical;
    noteAdded = note.code === ADDED_NOT_OPENED;
    set({ note: said(note) });
  }

  /**
   * What a saved take's landing said instead of landing in the editor, now that Klipolo is back: that it was added to a
   * project that couldn't be opened is news; anything else says it wasn't added, and where its files are.
   */
  function sayLanding(note: CodedError) {
    if (note.code === ADDED_NOT_OPENED) toast.info(said(note), { key: LANDING_TOAST });
    else toast.error(said(note), { key: LANDING_TOAST });
  }

  /**
   * Take `id` is over, and what it said while Klipolo was hidden is said. A take that recorded is saved (the app lands
   * it), so the dialog goes; one that never recorded leaves the choices. `freed`: its worker has gone (`state: idle`);
   * else Record waits for that.
   */
  function end(id: string, freed: boolean) {
    const saved = recorded === id;
    if (saved) recorded = null;
    // Saved, it lands next (a take given up while saving, `driveStuck`, too): what its landing says is still heard.
    if (saved && !freed) landing = id;
    if (freed) gone(id);
    else leave(id);
    if (started === id) started = null;
    press = null;
    set(NO_TAKE);
    sayNote();
    if (saved) get().close();
  }

  function countdown(left: number) {
    if (get().gate !== null) {
      if (gateFrom === null) gateFrom = left;
      else if (left < gateFrom) {
        gateFrom = null;
        set({ gate: null });
      }
    }
    set({ countdown: left });
  }

  function takeEvent(id: string, ev: CaptureEvent) {
    switch (ev.ev) {
      case 'state':
        // A take given up after recording, or finished from its files, ends here with no `finished`.
        if (ev.state === 'idle') return end(id, true);
        if (ev.state === 'recording' || ev.state === 'paused') {
          recorded = id;
          return set({ worker: ev.state, countdown: null, gate: null, meterDbfs: null });
        }
        return set({ worker: ev.state });
      case 'countdown':
        return countdown(ev.leftS);
      case 'gate':
        gateFrom = get().countdown;
        return set({ gate: ev.missing.length > 0 ? ev.missing : null });
      case 'meter':
        if (ev.source === MIC) set({ meterDbfs: ev.peakDbfs });
        return;
      case 'health':
        return keepNote(ev.note, ev.level);
      case 'finished':
        // Saved: the app lands it (`recorder-imported`) and the dialog goes.
        recorded = id;
        return end(id, false);
      case 'failed':
        // After the gate the take is still saved, and the dialog goes: why it stopped is said as its note. Before it,
        // the choices come back with the sentence.
        if (recorded === id) keepNote(ev.error, 'critical');
        else
          set({
            error: said(ev.error),
            errorCode: ev.error.code,
            errorFrom: 'take',
            ...(ev.error.code === SCREEN_REFUSED ? { screenRefused: true } : {}),
          });
        return end(id, false);
      // Heartbeats keep a slow save alive; frames and Start over aren't the dialog's.
      case 'heartbeat':
      case 'preview':
      case 'startOver':
        return;
    }
  }

  function hear(e: RecorderEvent) {
    if (e.take === landing && e.take !== get().take && e.event.ev === 'health' && e.event.level === 'critical')
      return sayLanding(e.event.note);
    if (leaving?.id === e.take) {
      if (e.event.ev === 'state' && e.event.state === 'idle') gone(e.take);
      return;
    }
    const { take } = get();
    if (take !== null && e.take === take) return takeEvent(take, e.event);
    // A take arming hasn't named yet may speak first: kept until it has (a level reading isn't worth keeping).
    if (press !== null && take === null && e.event.ev !== 'meter' && held.length < HELD_KEPT) held.push(e);
  }

  /** Arming named take `id`: what it said meanwhile is heard now, in order; anything else held is no one's. */
  function release(id: string) {
    const mine = held.filter((e) => e.take === id);
    held.length = 0;
    for (const e of mine) hear(e);
  }

  /**
   * Record: arms the take with the person's choices, then sends its Start. Nothing happens unless the dialog shows its
   * choices with something to record and Screen Recording allowed: a second press, a press during a take, and a press
   * while the Screen Recording step shows arm nothing.
   */
  async function record() {
    const s = get();
    if (!s.open || s.setup === null || phaseOf(s) !== 'choosing') return;
    if (screenStep(s.setup, s.picks, s.screenRefused)) return;
    const options = armOptions(s.picks);
    if (options === null) return;
    // The take's worker owns the screen, the camera and the mic from here: the dialog's previews let them go.
    stopPreviews();
    // A take whose worker said it failed hasn't gone yet: Record waits for it, as for any take let go.
    if (s.take !== null) leave(s.take);
    presses += 1;
    const mine: Press = { cancelled: false };
    press = mine;
    held.length = 0;
    gateFrom = null;
    noteCritical = false;
    noteAdded = false;
    set({ ...NO_TAKE, ...NO_ERROR, worker: 'starting', note: null });
    await lastTakeGone();
    if (mine.cancelled) return;
    let id: string;
    try {
      ({ id } = await recorder.arm(options));
    } catch (error) {
      if (mine.cancelled) return;
      press = null;
      held.length = 0;
      set({ ...NO_TAKE, ...refused(error, 'take') });
      return;
    }
    if (mine.cancelled) {
      // Cancelled while its worker was launched: it goes at once and never starts.
      held.length = 0;
      return letGo(id, 'disarm');
    }
    set({ take: id });
    release(id);
    // It ended before its Start (its worker failed, and said why).
    if (get().take !== id) return;
    started = id;
    try {
      await recorder.send(id, { cmd: 'start' });
    } catch (error) {
      if (press === mine) press = null;
      if (mine.cancelled || isCode(error, STALE)) return;
      set({ ...NO_TAKE, ...refused(error, 'take') });
      return letGo(id, 'disarm');
    }
    if (press === mine) press = null;
  }

  /**
   * Cancel (or Esc, or closing the dialog) from the press through the countdown: the take ends with nothing saved and
   * the choices come back at once. A press still arming never starts: its worker is let go as soon as it exists.
   */
  async function cancel() {
    const s = get();
    if (phaseOf(s) !== 'starting') return;
    if (press) {
      press.cancelled = true;
      press = null;
    }
    held.length = 0;
    // What a press in the countdown couldn't do goes with it: the choices come back as they were.
    set({ ...NO_TAKE, ...NO_ERROR });
    if (s.take !== null) await letGo(s.take, started === s.take ? 'cancel' : 'disarm');
  }

  /** "Record without it": the start gate stops waiting for `source`, and the take records the others. */
  async function recordWithout(source: string) {
    const { take } = get();
    if (take === null) return;
    try {
      await recorder.send(take, { cmd: 'withoutSource', source });
    } catch (error) {
      if (!isCode(error, STALE)) set(refused(error, 'press'));
      return;
    }
    if (get().take !== take) return;
    const left = (get().gate ?? []).filter((s) => s !== source);
    set({ gate: left.length > 0 ? left : null });
  }

  function closing() {
    const s = get();
    if (phaseOf(s) === 'starting') return void cancel();
    // Recording or paused: Stop saves it. While it saves there is nothing to send. Either way it is followed to its end.
    if (s.take !== null && (s.worker === 'recording' || s.worker === 'paused')) void tell(s.take, 'stop');
  }

  return {
    actions: { record, cancel, recordWithout },
    hear,
    closing,
    forget: () => {
      if (press) press.cancelled = true;
      press = null;
      started = null;
      recorded = null;
      leaving?.gone();
      leaving = null;
      held.length = 0;
      departed.clear();
      gateFrom = null;
      noteCritical = false;
      noteAdded = false;
      landing = null;
    },
  };
}
