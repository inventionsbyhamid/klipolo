import { create } from 'zustand';
import type { PrivacyPage } from '../bindings/PrivacyPage';
import type { RecorderSetup } from '../bindings/RecorderSetup';
import type { WorkerState } from '../bindings/WorkerState';
import { recorder, type TakeImport, type UnlistenFn } from '../bridge/recorder';
import { errorMessage } from '../lib/errors';
import { defaultPicks, NO_PICKS, type Picks } from '../lib/recorder/choices';
import { phaseOf } from '../lib/recorder/phase';
import { clipStarts } from '../lib/timeline';
import { usePlayback } from './playback';
import { useProject } from './project';
import { NO_TAKE, takeSlice, type TakeSlice } from './recorderTake';
import { useSelection } from './selection';

/** The Record dialog and its take (`phaseOf` says which of its views shows). */
export interface RecorderState {
  /** The dialog is open. It hides itself while its take records or saves, as Klipolo's window is hidden then. */
  open: boolean;
  /** Devices, permissions and notes, read once per opening; null while it is read. */
  setup: RecorderSetup | null;
  /** Why the setup couldn't be read, as a sentence ("Recording is off in this test run."). */
  setupError: string | null;
  /** The person's choices; the setup's defaults once it is here (`defaultPicks`). */
  picks: Picks;
  /** The take's id from arming until it ends; its events are the only ones heard. */
  take: string | null;
  /** The take's worker as its events say: `starting` from the Record press, null with no take. */
  worker: WorkerState | null;
  /** Seconds left before the take records. */
  countdown: number | null;
  /** The sources the start gate is still waiting for ("{source} isn't sending anything yet."). */
  gate: string[] | null;
  /** The mic's peak level during the countdown, in dBFS. */
  meterDbfs: number | null;
  /** Why the take didn't start, or why it stopped, as a sentence. */
  error: string | null;
  /**
   * "Allow screen recording" was pressed in this run of Klipolo: the step says to reopen Klipolo, which applies the
   * grant. Kept across openings.
   */
  askedScreen: boolean;
  /** A take in this opening was refused Screen Recording as it started: its step shows, whatever the setup read. */
  screenRefused: boolean;
  /**
   * What happened to the take while Klipolo's window was hidden (the encoder stopped it, the camera stopped), as a
   * sentence: kept until the window is back as the take ends, then said once as a toast.
   */
  note: string | null;
}

export interface RecorderStore extends RecorderState {
  /**
   * Opens the dialog and reads the setup: nothing is armed, asked or captured. Open already, it reads the setup again
   * only when it couldn't be read (Try again).
   */
  openRecorder: () => Promise<void>;
  /** Closes the dialog; during the countdown the take is cancelled first. */
  close: () => void;
  pick: (p: Partial<Picks>) => void;
  /** Record: arms the take, then starts it (its countdown). */
  record: () => Promise<void>;
  /** Cancel, from the press through the countdown: nothing is saved. */
  cancel: () => Promise<void>;
  /** "Record without it": the start gate stops waiting for `source`. */
  recordWithout: (source: string) => Promise<void>;
  /** "Allow screen recording": macOS asks, then its settings page opens; the step then says to reopen Klipolo. */
  allowScreen: () => Promise<void>;
  /** "Reopen Klipolo": saves, then starts Klipolo again. */
  reopen: () => Promise<void>;
  /** "Open privacy settings" for the screen, the camera or the microphone. */
  openPrivacy: (page: PrivacyPage) => Promise<void>;
}

/**
 * "Reopen Klipolo" ignores presses this long after it took the place of "Allow screen recording" under the pointer, so
 * a double click on Allow never reopens Klipolo.
 */
export const REOPEN_GRACE_MS = 1_000;

/** The dialog's part of the state when it is closed. */
const CLOSED = {
  open: false,
  setup: null,
  setupError: null,
  picks: NO_PICKS,
  error: null,
  screenRefused: false,
} satisfies Partial<RecorderState>;

const INITIAL: RecorderState = { ...CLOSED, ...NO_TAKE, askedScreen: false, note: null };

/** Each opening's setup is its own: one read for an earlier opening is never shown in a later one. */
let opening = 0;
/** The setup read on its way: an opening while it is waits for it, so the app never reads two at once. */
let reading: Promise<RecorderSetup> | null = null;
/** "Allow screen recording" is on its way: a second press does nothing. */
let asking = false;
/** When "Allow screen recording" answered, and Reopen took its place. */
let askedAt = Number.NEGATIVE_INFINITY;
/** The take's part of the store (its press, held events, the take it lets go). */
let takePart: TakeSlice;
/** The events followed, once. */
let following: Promise<UnlistenFn> | null = null;

function readSetup(): Promise<RecorderSetup> {
  if (reading) return reading;
  const mine: Promise<RecorderSetup> = recorder.setup().finally(() => {
    if (reading === mine) reading = null;
  });
  reading = mine;
  return mine;
}

/**
 * The Record dialog's store: its setup and the person's choices, the permission steps (each only on the person's
 * press), and the take from Record to its end (`recorderTake.ts`).
 */
export const useRecorder = create<RecorderStore>((set, get) => {
  takePart = takeSlice(set, get);

  /** One press to the app; a refusal is said in the dialog. True when it went. */
  async function press(run: () => Promise<void>): Promise<boolean> {
    try {
      await run();
      return true;
    } catch (error) {
      set({ error: errorMessage(error) });
      return false;
    }
  }

  return {
    ...INITIAL,
    ...takePart.actions,

    openRecorder: async () => {
      const s = get();
      if (s.open && s.setupError === null) return;
      const mine = ++opening;
      set({ ...CLOSED, open: true });
      try {
        const setup = await readSetup();
        if (mine === opening) set({ setup, picks: defaultPicks(setup) });
      } catch (error) {
        if (mine === opening) set({ setupError: errorMessage(error) });
      }
    },

    close: () => {
      const s = get();
      if (!s.open) return;
      // Nothing records without the dialog: a take on its way is cancelled first.
      if (phaseOf(s) === 'starting') void s.cancel();
      opening++;
      set(CLOSED);
    },

    pick: (p) => set((s) => ({ picks: { ...s.picks, ...p } })),

    allowScreen: async () => {
      if (asking) return;
      asking = true;
      try {
        if (await press(() => recorder.allowScreen())) {
          askedAt = Date.now();
          set({ askedScreen: true });
        }
      } finally {
        asking = false;
      }
    },

    reopen: async () => {
      if (Date.now() - askedAt < REOPEN_GRACE_MS) return;
      // The app answers before Klipolo goes: nothing more happens here.
      await press(() => recorder.reopen());
    },

    openPrivacy: async (page) => void (await press(() => recorder.openPrivacy(page))),
  };
});

/**
 * Follows the recorder once for the whole run: every take's events (only the dialog's take is heard) and every take
 * that lands. Resolves with the function that stops following.
 */
export function followRecorder(): Promise<UnlistenFn> {
  if (following) return following;
  const mine: Promise<UnlistenFn> = Promise.all([
    recorder.onEvent((event) => takePart.hear(event)),
    recorder.onImported((landed) => void onImported(landed)),
  ]).then(
    ([offEvents, offImported]) =>
      () => {
        offEvents();
        offImported();
        if (following === mine) following = null;
      },
    (error: unknown) => {
      if (following === mine) following = null;
      throw error;
    },
  );
  following = mine;
  return mine;
}

/** Back to as it was at launch: closed, no take, nothing followed, asked or waited for (tests). */
export function forgetRecorder() {
  takePart.forget();
  opening++;
  reading = null;
  asking = false;
  askedAt = Number.NEGATIVE_INFINITY;
  const followed = following;
  following = null;
  void followed?.then(
    (stop) => stop(),
    () => undefined,
  );
  useRecorder.setState(INITIAL);
}

/**
 * A take went into a project: the page takes the project as the app has it now (the open one, or the new one made
 * for a take from the start screen), selects the take's clip with the playhead at its start, and says so.
 */
export async function onImported(landed: TakeImport): Promise<void> {
  if (!(await useProject.getState().load(landed.snapshot))) return;
  const main = useProject.getState().snapshot?.project.timeline.main ?? [];
  const index = main.findIndex((clip) => clip.id === landed.clipId);
  if (index >= 0) {
    useSelection.getState().select(landed.clipId);
    usePlayback.getState().seek(clipStarts(main)[index]);
  }
  landedToast(landed);
}

/** What the person hears once a take has landed. It says nothing yet: its words come with the dialog's wiring. */
export function landedToast(_landed: TakeImport): void {}
