import { t } from '@lingui/core/macro';
import { create } from 'zustand';
import type { BackFromReopen } from '../bindings/BackFromReopen';
import type { LeftTake } from '../bindings/LeftTake';
import type { PrivacyPage } from '../bindings/PrivacyPage';
import type { Project } from '../bindings/Project';
import type { RecorderSetup } from '../bindings/RecorderSetup';
import type { WorkerState } from '../bindings/WorkerState';
import { codedMessage } from '../bridge/call';
import { recorder, type TakeImport, type UnlistenFn } from '../bridge/recorder';
import { errorMessage } from '../lib/errors';
import { defaultPicks, NO_PICKS, type Picks } from '../lib/recorder/choices';
import { formatDuration } from '../lib/time';
import { clipDurationUs, clipStarts, recordingName } from '../lib/timeline';
import { usePlayback } from './playback';
import { useProject } from './project';
import { forgetPreviews, hearPreview, resetPreviews } from './recorderPreviews';
import {
  NO_ERROR,
  NO_TAKE,
  refused,
  saidOnReturn,
  takeSlice,
  type ErrorFrom,
  type TakeSlice,
} from './recorderTake';
import { useSelection } from './selection';
import { toast } from './toasts';

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
  /**
   * macOS is asking the person something on the screen (a permission prompt, or its question about the private window
   * picker): the countdown waits, and starts again from the top once it is answered, so the question is never recorded.
   */
  systemAlert: boolean;
  /** The mic's peak level during the countdown, in dBFS. */
  meterDbfs: number | null;
  /** Why the take didn't start, or why it stopped, or why a press was refused, as a sentence. */
  error: string | null;
  /** The refusal's code (`capture.permission.mic`), when it came with one: what the dialog offers follows it. */
  errorCode: string | null;
  /**
   * Who was refused: a `take` (Record or Try again; the dialog offers Try again) or a `press` (a permission button,
   * Record without it, a Cancel that couldn't be sent; pressing it again is the way).
   */
  errorFrom: ErrorFrom | null;
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
  /**
   * A take let go whose worker hasn't gone yet, so the app still has it: `adding` when it was saved and is landing,
   * `stopping` when it was cancelled or failed. Record waits for it; null once it has gone.
   */
  lastTake: LastTake | null;
}

/** What a take let go is doing until its worker has gone (`RecorderState.lastTake`). */
export type LastTake = 'adding' | 'stopping';

export interface RecorderStore extends RecorderState {
  /**
   * Opens the dialog and reads the setup: nothing is armed, asked or captured, and playback pauses (its sound would
   * reach the countdown and the mic). Open already, it resolves once the setup on its way is read, and reads it again
   * only when it couldn't be (Try again). While a take runs it does nothing.
   */
  openRecorder: () => Promise<void>;
  /**
   * Closes the dialog and stops its previews. From the press through the countdown the take is cancelled first; while
   * it records, it is stopped and saved, and followed to its end.
   */
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
  ...NO_ERROR,
  screenRefused: false,
} satisfies Partial<RecorderState>;

const INITIAL: RecorderState = { ...CLOSED, ...NO_TAKE, askedScreen: false, note: null, lastTake: null };

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
  takePart = takeSlice(set, get, () => void lastTakeWent());

  /**
   * The last take's worker has gone: a setup read while the app still had it (Record off, "Klipolo is adding your last
   * recording.") is read again, so Record turns on. The person's choices stay.
   */
  async function lastTakeWent() {
    const mine = opening;
    // A setup on its way may have been read before the take went.
    await reading?.catch(() => undefined);
    const s = get();
    if (mine !== opening || !s.open || s.take !== null || !s.setup?.ownTake) return;
    try {
      const setup = await readSetup();
      if (mine === opening && get().take === null) set({ setup });
    } catch {
      // What shows stays; closing and opening the dialog reads it again.
    }
  }

  /** One press to the app; a refusal is said in the dialog. True when it went. */
  async function press(run: () => Promise<void>): Promise<boolean> {
    try {
      await run();
      return true;
    } catch (error) {
      set(refused(error, 'press'));
      return false;
    }
  }

  return {
    ...INITIAL,
    ...takePart.actions,

    openRecorder: async () => {
      const s = get();
      // A take runs (its dialog hidden, or closed while it records): nothing new opens and nothing changes.
      if (s.take !== null) return;
      usePlayback.getState().pause();
      // Open already: done once the setup on its way is read.
      if (s.open && s.setupError === null)
        return reading?.then(
          () => undefined,
          () => undefined,
        );
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
      if (!get().open) return;
      forgetPreviews();
      takePart.closing();
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
 * Follows the recorder once for the whole run: the Record dialog's previews' events, every take's events (only the
 * dialog's take is heard), every take that lands, and what became of takes the last run left behind that didn't.
 * Resolves with the function that stops following.
 */
export function followRecorder(): Promise<UnlistenFn> {
  if (following) return following;
  const mine: Promise<UnlistenFn> = Promise.all([
    recorder.onEvent((event) => void (hearPreview(event) || takePart.hear(event))),
    recorder.onImported((landed) => void onImported(landed)),
    recorder.onLeft(leftToast),
  ]).then(
    ([offEvents, offImported, offLeft]) =>
      () => {
        offEvents();
        offImported();
        offLeft();
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

/**
 * At launch: when Klipolo has just opened again through Reopen Klipolo, which the person pressed while choosing what to
 * record, the project they had open opens again, then the Record dialog. Resolves once the project is open (the
 * dialog's setup is read after); a project that can't open says why, and the dialog stays closed.
 */
export async function reopenWhereLeft(): Promise<void> {
  let back: BackFromReopen | null;
  try {
    back = await recorder.backFromReopen();
  } catch {
    return;
  }
  if (back === null) return;
  if (back.project !== null && !(await useProject.getState().open(back.project))) return;
  void useRecorder.getState().openRecorder();
}

/**
 * At launch, once the page follows the recorder: takes the last run left behind (Klipolo quit or crashed during them)
 * land, each in a new project, heard as any landed take is and told as from the last session. The app looks once per
 * run, on a thread of its own; a test run has none.
 */
export async function recoverLeftTakes(): Promise<void> {
  try {
    await followRecorder();
    await recorder.recoverLeft();
  } catch {
    // Nothing to land: the takes stay in their folders for the next launch.
  }
}

/**
 * What the person hears about a take the last run left behind that didn't land: it couldn't be added, its new project
 * couldn't be opened, or it is still recording. One toast for each of these, however many takes say it.
 */
export function leftToast(left: LeftTake): void {
  const text = {
    notAdded: t({
      id: 'recorder.left.notAdded',
      message: "A recording from your last session couldn't be added. Its files are in Movies / Klipolo / Recordings.",
      comment: "At launch, a recording Klipolo closed during couldn't be added to a project",
    }),
    addedNotOpened: t({
      id: 'recorder.left.addedNotOpened',
      message: 'A recording from your last session was added to a new project. Open it from the start screen to see it.',
      comment: "At launch, a recording Klipolo closed during was added to a new project, which couldn't be opened",
    }),
    stillRecording: t({
      id: 'recorder.left.stillRecording',
      message: 'A recording from your last session is still going. Stop it from the menu bar to add it here.',
      comment: 'At launch, a recording Klipolo closed during still records, stopped from its menu bar item',
    }),
  }[left.outcome];
  toast.info(text, { key: `recorder-left-${left.outcome}` });
}

/**
 * Record your screen, wherever it is offered (the start screen, the Library's head, an empty project's preview): opens
 * the Record dialog, as Shift Command R does.
 */
export function recordYourScreen() {
  void useRecorder.getState().openRecorder();
}

/** Back to as it was at launch: closed, no take, nothing followed, asked or waited for (tests). */
export function forgetRecorder() {
  takePart.forget();
  resetPreviews();
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

/** The undo step a take's auto zoom is: "Add auto zoom". */
export const AUTO_ZOOM = 'undo.autoZoom';
/** The toast that says a take landed: a later take's takes its place. */
const LANDED_TOAST = 'recorder-landed';

/**
 * A take went into a project: the page takes the project as the app has it now, selects the take's clip with the
 * playhead at its start, and says so. The project the page shows takes it as a change from elsewhere; another one (the
 * one the take was armed in, opened again; a new one made for a take from the start screen), or the same one opened
 * again, takes the page's place as any switch does, the edits made in the one shown going in first.
 */
export async function onImported(landed: TakeImport): Promise<void> {
  const project = useProject.getState();
  const shown = project.snapshot;
  const next = landed.snapshot;
  const same = shown !== null && shown.project.id === next.project.id && shown.opening === next.opening;
  if (!(await (same ? project.load(next) : project.switchToOpened(next)))) return;
  const main = useProject.getState().snapshot?.project.timeline.main ?? [];
  const index = main.findIndex((clip) => clip.id === landed.clipId);
  if (index >= 0) {
    useSelection.getState().select(landed.clipId);
    usePlayback.getState().seek(clipStarts(main)[index]);
  }
  landedToast(landed);
}

/**
 * What the person hears once a take has landed: "Added Recording 1, 0:40.", or for a take the last run left behind,
 * "Recovered Recording 1 from your last session."; with its zooms, "Zooms follow your clicks." with Remove zooms. Then
 * what the take's notes say, each once, but for those said already as Klipolo came back.
 */
export function landedToast(landed: TakeImport): void {
  const project = landed.snapshot.project;
  const clip = project.timeline.main.find((c) => c.id === landed.clipId);
  if (clip) {
    const name = recordingName(project, landed.recordingId);
    const duration = formatDuration(clipDurationUs(clip));
    const zooms = landed.zooms > 0;
    const text = landed.earlierRun ? recoveredText(name, zooms) : addedText(name, duration, zooms);
    const remove = {
      label: t({ id: 'recorder.removeZooms', message: 'Remove zooms' }),
      run: () => void removeZooms(project.id, landed.recordingId),
    };
    toast.info(text, { key: LANDED_TOAST, done: true, actions: zooms ? [remove] : undefined });
  }
  for (const note of landed.notes) {
    const text = codedMessage(note);
    if (text && !saidOnReturn(note.code)) toast.info(text);
  }
}

/** A take of this run landed: "Added Recording 1, 0:40.", with "Zooms follow your clicks." when it has `zooms`. */
function addedText(name: string, duration: string, zooms: boolean): string {
  return zooms
    ? t({
        id: 'recorder.landed.zooms',
        message: `Added ${name}, ${duration}. Zooms follow your clicks.`,
        comment: 'A recording was added to the project, with zooms on its clicks: {name} is Recording 1',
      })
    : t({
        id: 'recorder.landed',
        message: `Added ${name}, ${duration}.`,
        comment: 'A recording was added to the project: {name} is Recording 1, {duration} 0:40',
      });
}

/**
 * A take the last run left behind (Klipolo quit or crashed during it) landed in a new project as Klipolo opened:
 * "Recovered Recording 1 from your last session.", with "Zooms follow your clicks." when it has `zooms`.
 */
function recoveredText(name: string, zooms: boolean): string {
  return zooms
    ? t({
        id: 'recorder.recovered.zooms',
        message: `Recovered ${name} from your last session. Zooms follow your clicks.`,
        comment: 'At launch, a take Klipolo closed during went into a new project, with zooms: {name} is Recording 1',
      })
    : t({
        id: 'recorder.recovered',
        message: `Recovered ${name} from your last session.`,
        comment: 'At launch, a recording Klipolo closed during went into a new project: {name} is Recording 1',
      });
}

/** Whether `project` still has a zoom attached to recording `recordingId` (its auto zoom's, or one added since). */
function hasZoomsOn(project: Project, recordingId: string): boolean {
  return project.timeline.tracks.some((track) =>
    track.items.some(
      (item) =>
        item.content.type === 'zoom' &&
        item.attach?.source.type === 'recording' &&
        item.attach.source.id === recordingId,
    ),
  );
}

/**
 * Remove zooms: take `recordingId`'s auto zoom step is undone while it is still the last change in its project, and
 * only then. When its zooms are gone already (an Undo took them back, or they were deleted), it says so; after an edit
 * on top, nothing is undone, and the person is told to delete them on the timeline.
 */
export async function removeZooms(projectId: string, recordingId: string): Promise<void> {
  const ours = () => useProject.getState().snapshot?.project.id === projectId;
  const undone = await useProject.getState().undoIf((history) => history.undoLabel === AUTO_ZOOM && ours());
  if (undone) return;
  const project = useProject.getState().snapshot?.project;
  if (project?.id === projectId && !hasZoomsOn(project, recordingId))
    toast.info(
      t({
        id: 'recorder.zoomsGone',
        message: 'The zooms are already gone.',
        comment: "Remove zooms on a recording's toast, after its zooms were undone or deleted",
      }),
    );
  else
    toast.info(
      t({
        id: 'recorder.zoomsKept',
        message: "Klipolo can't remove them now. Delete them on the timeline.",
        comment: "Remove zooms on a recording's toast, after a later change",
      }),
    );
}
