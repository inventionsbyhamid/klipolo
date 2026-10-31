import { create } from 'zustand';
import type { MediaJobInfo } from '../bindings/MediaJobInfo';
import type { MediaJobList } from '../bindings/MediaJobList';
import type { MediaJobsHold } from '../bindings/MediaJobsHold';
import type { PauseBy } from '../bindings/PauseBy';
import { mediaJobs } from '../bridge/mediaJobs';
import { copyWhy, refusalKind, refusalText, type RefusalKind } from './mediaJobReasons';
import { toast } from './toasts';
import { useUi } from './ui';

export { copyWhy, type CopyWhy } from './mediaJobReasons';

/**
 * A list that only moves progress is drawn at most this often (Rust sends one at most every 250 ms); one that changes
 * a row's state, its reason, the files or who holds copies is drawn at once.
 */
export const PROGRESS_DRAW_MS = 500;

/** Paused only while the video plays, an export runs or a take records: it goes on by itself. */
export function pausedForAWhile(job: MediaJobInfo): boolean {
  if (job.state !== 'paused') return false;
  const why = copyWhy(job);
  return why === 'playing' || why === 'exporting' || why === 'recording';
}

/**
 * A copy the pill counts: being made, paused, or waiting its turn. With automatic copies off nothing waiting starts,
 * so a waiting row isn't counted: Rust drops those rows when copies go off and lists none while they are off, and
 * this covers the moment before its list says so (the switch moves at once; Rust starts with copies on until the
 * editor sends the person's setting).
 */
export function isActive(job: MediaJobInfo, auto: boolean): boolean {
  return job.state === 'working' || job.state === 'paused' || (job.state === 'waiting' && auto);
}

/**
 * A failed copy the person hasn't dismissed: it keeps the pill (quietly) until it is tried again or dismissed. Not
 * while copies are off: nothing would make its copy (Rust drops failed rows then too; this covers the moment before
 * its list says so).
 */
export function isFailedShown(job: MediaJobInfo, dismissed: readonly string[], auto: boolean): boolean {
  return auto && job.state === 'failed' && !dismissed.includes(job.path);
}

/**
 * A row the popover lists: every copy but finished ones and dismissed failures, and waiting ones only while copies
 * are automatic. Stopped copies are listed while the popover is open (Try again), but never keep the pill.
 */
export function isListed(job: MediaJobInfo, auto: boolean, dismissed: readonly string[]): boolean {
  if (job.state === 'done' || (job.state === 'waiting' && !auto)) return false;
  return job.state !== 'failed' || isFailedShown(job, dismissed, auto);
}

/** Progress (0 to 1) in whole percent, never 100 before it is: 0.999 is 99. */
export function wholePercent(progress: number): number {
  return Math.min(100, Math.max(0, Math.floor(progress * 100 + 1e-9)));
}

/** How many copies the pill counts. */
export function activeCount(jobs: MediaJobInfo[], auto: boolean): number {
  let count = 0;
  for (const job of jobs) if (isActive(job, auto)) count++;
  return count;
}

/** The counted copies' mean progress, in whole percent (0 when there are none). */
export function activePercent(jobs: MediaJobInfo[], auto: boolean): number {
  let count = 0;
  let sum = 0;
  for (const job of jobs)
    if (isActive(job, auto)) {
      count++;
      sum += job.progress;
    }
  return count === 0 ? 0 : wholePercent(sum / count);
}

/** Failed copies shown (not dismissed, copies on). */
export function failedCount(jobs: MediaJobInfo[], dismissed: readonly string[], auto: boolean): number {
  let count = 0;
  for (const job of jobs) if (isFailedShown(job, dismissed, auto)) count++;
  return count;
}

/** Says a refusal as a toast (the pill and its popover). */
export function sayRefusal(refused: string | null) {
  if (refused !== null) toast.error(refused);
}

interface MediaJobsStore {
  /** The open project's editing copies, as Rust last listed them (drawn: progress throttled). */
  jobs: MediaJobInfo[];
  /** Failed copies the person dismissed (their paths), until they leave the failed state or the editor closes. */
  dismissed: string[];
  /** The person paused copies from the pill. Rust holds the pause and lists it (`held`), so a reload knows it too. */
  pausedByPerson: boolean;
  /** An action paused them (`by: "action"`); the person's Resume all doesn't undo it. */
  pausedByAction: boolean;
  /** Lists the copies (the editor opened). A failure keeps the list, and an answer older than an event is dropped. */
  load: () => Promise<void>;
  /** Follows Rust's list until the returned function is called, which empties it. */
  listen: () => () => void;
  /**
   * A list from Rust, and what holds copies when it says (`held`): drawn at once when a row's state, its reason, the
   * files or the holds changed, else throttled (its rows only, so a list held back never undoes a pause or resume
   * answered meanwhile).
   */
  receive: (jobs: MediaJobInfo[], held?: MediaJobsHold[]) => void;
  /** Hides a failed copy (it no longer keeps the pill) until it fails again. */
  dismiss: (path: string) => void;
  /**
   * Each resolves with Rust's refusal in plain words, or null once Rust took it. A command Rust answered late (a slow
   * drive) is still applied once the drive answers, so it counts as taken: the flags follow it.
   */
  pause: (by: PauseBy) => Promise<string | null>;
  resume: (by: PauseBy) => Promise<string | null>;
  stop: (path: string) => Promise<string | null>;
  retry: (path: string) => Promise<string | null>;
  /**
   * Saves the person's setting and tells Rust now (it is also sent when the editor opens and when the engine
   * attaches). Turned back when Rust can't take it at all (`unavailable`), so the switch never shows what Rust doesn't.
   */
  setAuto: (on: boolean) => Promise<string | null>;
}

/**
 * The newest progress-only list not drawn yet (its rows only: its holds were the ones shown, so drawing it never
 * applies them again over a command's answer heard meanwhile), its timer, and when a list was last drawn (ms).
 */
let pending: MediaJobInfo[] | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;
let drawnAt = -Infinity;
/** Lists heard from Rust: a `load` that started before one is older than it. */
let heard = 0;

/** The same files in the same states with the same reasons: only progress differs. */
function sameRows(a: MediaJobInfo[], b: MediaJobInfo[]): boolean {
  return (
    a.length === b.length &&
    a.every(
      (job, i) =>
        job.path === b[i].path && job.state === b[i].state && job.reason?.code === b[i].reason?.code,
    )
  );
}

function cancelPending() {
  clearTimeout(timer);
  timer = undefined;
  pending = null;
}

/** Runs a command: what refused it, or null once Rust took it (now, or once a slow drive answers). */
async function command(run: () => Promise<void>): Promise<TakenOrNot> {
  try {
    await run();
    return null;
  } catch (error) {
    const refused = refusalKind(error);
    return refused === 'slowDrive' ? null : refused;
  }
}

type TakenOrNot = Exclude<RefusalKind, 'slowDrive'> | null;

const said = (refused: TakenOrNot) => (refused === null ? null : refusalText(refused));

export const useMediaJobs = create<MediaJobsStore>()((set, get) => {
  const draw = (jobs: MediaJobInfo[], held?: MediaJobsHold[]) => {
    cancelPending();
    drawnAt = Date.now();
    // A dismissal lasts while its copy stays failed: tried again (or gone), it can fail anew and be shown.
    const { dismissed } = get();
    const kept = dismissed.filter((path) => jobs.some((job) => job.path === path && job.state === 'failed'));
    const holds = held && {
      pausedByPerson: held.includes('person'),
      pausedByAction: held.includes('action'),
    };
    set({ jobs, ...holds, ...(kept.length === dismissed.length ? {} : { dismissed: kept }) });
  };
  /** Rust's holds differ from what this window shows. */
  const holdsMoved = (held?: MediaJobsHold[]) =>
    held !== undefined &&
    (held.includes('person') !== get().pausedByPerson || held.includes('action') !== get().pausedByAction);
  return {
    jobs: [],
    dismissed: [],
    pausedByPerson: false,
    pausedByAction: false,
    load: async () => {
      const before = heard;
      try {
        const list: MediaJobList | undefined = await mediaJobs.list();
        if (heard === before && list && Array.isArray(list.jobs)) draw(list.jobs, list.held);
      } catch {
        // No Tauri (tests, the browser's fake backend) or Rust busy: the list stays as it is; events still come.
      }
    },
    listen: () => {
      let gone = false;
      let unlisten: (() => void) | undefined;
      void (async () => {
        try {
          const off = await mediaJobs.onChange((list) => {
            if (!gone) get().receive(list.jobs, list.held);
          });
          if (gone) off();
          else unlisten = off;
        } catch {
          // Nothing to hear without Tauri.
        }
      })();
      return () => {
        gone = true;
        unlisten?.();
        cancelPending();
        set({ jobs: [], dismissed: [] });
      };
    },
    receive: (jobs, held) => {
      heard++;
      if (!sameRows(get().jobs, jobs) || holdsMoved(held)) {
        draw(jobs, held);
        return;
      }
      pending = jobs;
      // Clamped, so a clock set back never holds progress for long.
      const wait = Math.min(PROGRESS_DRAW_MS, Math.max(0, drawnAt + PROGRESS_DRAW_MS - Date.now()));
      timer ??= setTimeout(() => {
        if (pending) draw(pending);
      }, wait);
    },
    dismiss: (path) => {
      const { dismissed } = get();
      if (!dismissed.includes(path)) set({ dismissed: [...dismissed, path] });
    },
    pause: async (by) => {
      const refused = await command(() => mediaJobs.pause(by));
      if (refused === null) set(by === 'person' ? { pausedByPerson: true } : { pausedByAction: true });
      return said(refused);
    },
    resume: async (by) => {
      const refused = await command(() => mediaJobs.resume(by));
      if (refused === null) set(by === 'person' ? { pausedByPerson: false } : { pausedByAction: false });
      return said(refused);
    },
    stop: async (path) => said(await command(() => mediaJobs.stop(path))),
    retry: async (path) => said(await command(() => mediaJobs.retry(path))),
    setAuto: async (on) => {
      const ui = useUi.getState();
      const before = ui.autoCopies;
      ui.setAutoCopies(on);
      const refused = await command(() => mediaJobs.setAuto(on));
      if (refused === 'unavailable' && useUi.getState().autoCopies === on) ui.setAutoCopies(before);
      return said(refused);
    },
  };
});
