import { create } from 'zustand';
import type { CaptionFile } from '../bindings/CaptionFile';
import type { ExportedFile } from '../bindings/ExportedFile';
import type { ExportEvent } from '../bindings/ExportEvent';
import type { ExportJobInfo } from '../bindings/ExportJobInfo';
import type { ExportQuality } from '../bindings/ExportQuality';
import type { FormatKind } from '../bindings/FormatKind';
import type { ResumeMode } from '../bindings/ResumeMode';
import { isCode } from '../bridge/call';
import { exporter } from '../bridge/exporter';
import { errorMessage } from '../lib/errors';
import { flushBursts } from './bursts';
import { commitsDone } from './commitQueue';
import {
  applyEvent,
  exportedText,
  idleRun,
  pauseRefusal,
  refusalReason,
  resuming,
  unsavedFormats,
  type ExportRun,
} from './exportRun';
import { settled, useProject } from './project';
import { toast } from './toasts';

export { canStartOver, exportPercent, shownJob, unfinishedJobs, type ExportPhase } from './exportRun';

/** "Also save captions as a file": whether, and as which kind. Kept on this Mac, whatever the project. */
export interface CaptionFileChoice {
  save: boolean;
  kind: CaptionFile;
}

/** What an export asks for besides its formats and quality. */
export interface StartOptions {
  /** Also save the captions as this kind of file beside the 16:9 video. */
  captionFile?: CaptionFile | null;
  /** Start over: the unfinished job this export replaces, which Rust discards once the new export is ready to run. */
  replaces?: string | null;
}

interface ExportStore extends ExportRun {
  /** The export dialog is on screen. */
  open: boolean;
  /** The quality the dialog has picked: High unless the person picks another. */
  quality: ExportQuality;
  /** The formats the person checked in the dialog; null: the format the preview shows, until they pick. */
  picked: FormatKind[] | null;
  /** Whether a 16:9 export also saves its captions as a file, and which kind (off, SRT, until chosen). Kept. */
  captionFile: CaptionFileChoice;
  /** What the running (or last) export asked for: Try again asks the same, less what is in place. */
  formats: FormatKind[];
  runQuality: ExportQuality;
  runCaptionFile: CaptionFile | null;
  /** Start over's kept job, which this export replaces once Rust has started it: Try again asks the same. */
  replaces: string | null;
  /** Unfinished exports on disk (`export_jobs`), newest first. */
  jobs: ExportJobInfo[];
  /** A Start over is under way (its plan, then its start): another press is ignored. */
  startingOver: boolean;
  /** The project the run on screen exports: Try again never exports another one. */
  projectId: string | null;
  /** Where the files go, as the dialog names it ("Movies / Klipolo"), once a plan or Change has said. */
  folder: string | null;
  /** The finished files with their sizes on the disk, once the run has ended with some in place. */
  written: ExportedFile[];
  setOpen: (open: boolean) => void;
  setQuality: (quality: ExportQuality) => void;
  setPicked: (formats: FormatKind[]) => void;
  setCaptionFile: (patch: Partial<CaptionFileChoice>) => void;
  setFolder: (folder: string) => void;
  /** Asks for the folder the files go to (Change); true when one was picked. */
  pickFolder: () => Promise<boolean>;
  /** Exports `formats` of the open project, every edit already made included; `captionFile` beside the 16:9 one. */
  start: (formats: FormatKind[], quality?: ExportQuality, options?: StartOptions) => Promise<void>;
  /** Resumes the kept job of a run that failed or paused; a run that never got a job starts again. */
  retry: () => Promise<void>;
  /** Stops the run on screen. Resolves on Rust's answer: null, or why not (worded). */
  cancel: () => Promise<string | null>;
  /** Pauses the running job `jobId` (a row's Pause). Resolves on Rust's answer: null, or why not (worded). */
  pause: (jobId: string) => Promise<string | null>;
  /** Continues a kept job from its finished parts; `compatible` finishes it in compatibility mode. */
  resume: (jobId: string, mode?: ResumeMode) => Promise<void>;
  /** Deletes an unfinished job and its parts; false if it couldn't be (said in a toast). */
  discard: (jobId: string) => Promise<boolean>;
  /**
   * A job whose project changed: exports its formats still in the project at its quality, from the project as it is
   * now, in its place (Rust discards it once the new export is ready to run, never before).
   */
  startOver: (job: ExportJobInfo) => Promise<void>;
  /** Reads the unfinished exports again. */
  loadJobs: () => Promise<void>;
  onEvent: (event: ExportEvent) => void;
  reset: () => void;
}

/** Where the captions file choice is kept (local storage, as JSON). */
const CAPTION_FILE = 'klipolo.captionFile';
const NO_CAPTION_FILE: CaptionFileChoice = { save: false, kind: 'srt' };

/** The captions file choice as the person left it; off, SRT, when nothing was kept or storage is refused. */
function keptCaptionFile(): CaptionFileChoice {
  try {
    const kept: unknown = JSON.parse(localStorage.getItem(CAPTION_FILE) ?? 'null');
    if (typeof kept !== 'object' || kept === null) return NO_CAPTION_FILE;
    const { save, kind } = kept as Partial<CaptionFileChoice>;
    return { save: save === true, kind: kind === 'vtt' ? 'vtt' : 'srt' };
  } catch {
    return NO_CAPTION_FILE;
  }
}

/**
 * The captions file an export of `formats` saves: the kind the person chose, when they chose to save one, the dialog
 * offers it (`offered`: some clip's words are heard) and a 16:9 video is among them; else none.
 */
export function chosenCaptionFile(formats: FormatKind[], offered: boolean): CaptionFile | null {
  const { save, kind } = useExport.getState().captionFile;
  return save && offered && formats.includes('landscape') ? kind : null;
}

/**
 * Shows the last export's first file (or file `index`) in Finder; a file that is gone says so in a toast (Rust's
 * `exports.revealGone`, worded by the bridge).
 */
export function revealExported(index?: number): Promise<void> {
  const shown = index === undefined ? exporter.reveal() : exporter.reveal(index);
  return shown.catch((error: unknown) => toast.error(errorMessage(error)));
}

/**
 * Shows the share menu for the last export's files beside `button` (Share…); files that are gone, or a menu that can't
 * show, say so in a toast.
 */
export function shareExported(button: Element): Promise<void> {
  const { x, y, width, height } = button.getBoundingClientRect();
  return exporter.share({ x, y, width, height }).catch((error: unknown) => toast.error(errorMessage(error)));
}

/** Opens file `index` of the last export in its app; one that is gone says so in a toast. */
export function openExported(index: number): Promise<void> {
  return exporter.open(index).catch((error: unknown) => toast.error(errorMessage(error)));
}

/** Events of an earlier run are ignored. */
let batch = 0;
/** Only the latest job list counts. */
let listing = 0;

const openProjectId = () => useProject.getState().snapshot?.project.id ?? null;
const ENDS = new Set<ExportEvent['type']>(['finished', 'paused', 'failed', 'cancelled']);

export const useExport = create<ExportStore>((set, get) => {
  const listener = (id: number) => (event: ExportEvent) => {
    if (id === batch) get().onEvent(event);
  };
  /** The sizes of the files in place, for the finished dialog's rows. */
  const readWritten = async (id: number) => {
    try {
      const written = await exporter.files();
      if (id === batch) set({ written });
    } catch {
      // The rows show the sizes they know.
    }
  };
  return {
    open: false,
    quality: 'high',
    picked: null,
    captionFile: keptCaptionFile(),
    formats: [],
    runQuality: 'high',
    runCaptionFile: null,
    replaces: null,
    jobs: [],
    startingOver: false,
    projectId: null,
    folder: null,
    written: [],
    ...idleRun,
    setOpen: (open) => {
      if (open) {
        // The status region mounts empty: it announces what happens next, not what already happened. A fresh dialog
        // starts from the format the preview shows.
        const fresh = get().phase === 'idle';
        set({ open, announcement: '', ...(fresh ? { picked: null } : {}) });
        void get().loadJobs();
        return;
      }
      set({ open });
      // A run that ended is put away with the dialog (a kept job joins the unfinished list); one still running carries
      // on (the top bar shows it).
      const { phase } = get();
      if (phase === 'done' || phase === 'failed' || phase === 'cancelled' || phase === 'paused') get().reset();
    },
    setQuality: (quality) => set({ quality }),
    setPicked: (picked) => set({ picked }),
    setCaptionFile: (patch) => {
      const captionFile = { ...get().captionFile, ...patch };
      try {
        localStorage.setItem(CAPTION_FILE, JSON.stringify(captionFile));
      } catch {
        // Storage refused: the choice holds for this session.
      }
      set({ captionFile });
    },
    setFolder: (folder) => set({ folder }),
    pickFolder: async () => {
      try {
        const folder = await exporter.pickFolder();
        if (folder === null) return false;
        set({ folder });
        return true;
      } catch (error) {
        toast.error(errorMessage(error));
        return false;
      }
    },
    start: async (formats, quality = get().quality, { captionFile = null, replaces = null } = {}) => {
      const id = ++batch;
      set({
        ...idleRun,
        phase: 'running',
        formats,
        runQuality: quality,
        runCaptionFile: captionFile,
        replaces,
        projectId: openProjectId(),
        written: [],
      });
      // A toolbar change still settling belongs in this export, and so does every edit already made: those still
      // waiting their turn in the commit queue (a slider released just before), then those already sent.
      flushBursts();
      const slow = setTimeout(() => id === batch && set({ saving: true }), 400);
      try {
        const open = await commitsDone();
        if (id !== batch) return;
        // The project closed first: there is nothing to export.
        if (!open) return get().onEvent({ type: 'cancelled' });
        await settled();
        if (id !== batch) return;
        // Stopped before it reached Rust: nothing to undo.
        if (get().stopping) return get().onEvent({ type: 'cancelled' });
        const asked = { ...(captionFile ? { captionFile } : {}), ...(replaces ? { replaces } : {}) };
        await (Object.keys(asked).length > 0
          ? exporter.start(formats, quality, listener(id), asked)
          : exporter.start(formats, quality, listener(id)));
        // Start over: Rust has discarded the kept job, so its row goes.
        if (replaces) void get().loadJobs();
        // Stopped while Rust was taking the export slot: said again now that it holds it.
        if (id === batch && get().stopping && get().phase === 'running') void get().cancel();
      } catch (error) {
        if (id === batch) get().onEvent({ type: 'failed', message: errorMessage(error) });
      } finally {
        clearTimeout(slow);
        if (id === batch) set({ saving: false });
      }
    },
    retry: () => {
      const { jobId, files, ready, formats, projectId, runQuality, runCaptionFile, replaces } = get();
      // Its job is kept with every finished part: it goes on from there.
      if (jobId) return get().resume(jobId);
      // A run that never got a job is its project's: in another project it is gone, not exported again.
      if (projectId !== openProjectId()) {
        get().reset();
        return Promise.resolve();
      }
      // The files already in place stay: only what wasn't saved is made again.
      const again = files.length > 0 ? unsavedFormats({ files, ready }) : formats;
      const next = again.length > 0 ? again : formats;
      // The captions file goes with the 16:9 video: asked again only when that is made again. Start over still
      // replaces its kept job, which a refused start kept.
      const captionFile = next.includes('landscape') ? runCaptionFile : null;
      return get().start(next, runQuality, { captionFile, replaces });
    },
    cancel: () => {
      const own = get().phase === 'running';
      if (own) set({ stopping: true });
      // The run on screen is named by its job once it has one. Without a job, Rust stops only the open project's
      // export and refuses another project's by name (worded here, `refusalReason`).
      const job = own ? get().jobId : null;
      return (job ? exporter.cancel(job) : exporter.cancel()).then(
        () => null,
        (error: unknown) => {
          // Not stopped: Cancel export is offered again.
          if (own) set({ stopping: false });
          return refusalReason(error);
        },
      );
    },
    pause: (jobId) =>
      exporter.pause(jobId).then(
        () => {
          // Its events don't reach this window (it was started before a reload): its row shows it paused now, and
          // Resume waits for it to finish its current part.
          set((s) => ({ jobs: s.jobs.map((j) => (j.jobId === jobId ? { ...j, state: 'paused' } : j)) }));
          return null;
        },
        (error: unknown) => {
          // The list may be out of date (it ended): it is read again.
          void get().loadJobs();
          return pauseRefusal(error);
        },
      ),
    resume: async (jobId, mode) => {
      const id = ++batch;
      const s = get();
      const job = s.jobs.find((j) => j.jobId === jobId);
      set({
        ...resuming(s, jobId, job),
        projectId: job?.projectId ?? s.projectId,
        formats: job ? job.files.map((f) => f.format) : s.formats,
        runQuality: job?.quality ?? s.runQuality,
        written: [],
      });
      try {
        await (mode ? exporter.resume(jobId, listener(id), mode) : exporter.resume(jobId, listener(id)));
      } catch (error) {
        if (id !== batch) return;
        // Gone meanwhile (discarded elsewhere): Try again exports it anew rather than asking for it again.
        if (isCode(error, 'exports.gone')) set({ jobId: null });
        get().onEvent({ type: 'failed', message: errorMessage(error) });
      }
    },
    discard: async (jobId) => {
      try {
        await exporter.discard(jobId);
      } catch (error) {
        toast.error(errorMessage(error));
        void get().loadJobs();
        return false;
      }
      if (get().jobId === jobId) get().reset();
      set((s) => ({ jobs: s.jobs.filter((j) => j.jobId !== jobId) }));
      void get().loadJobs();
      return true;
    },
    startOver: async (job) => {
      if (get().startingOver) return;
      const inProject = useProject.getState().snapshot?.project.settings.formats ?? [];
      const formats = job.files.map((f) => f.format).filter((f) => inProject.includes(f));
      // None of its formats is in the project any more: formats are chosen again.
      if (formats.length === 0) return get().reset();
      set({ startingOver: true });
      try {
        // The plan says whether a captions file can go with it; without an answer nothing is thrown away.
        let offered: boolean;
        try {
          offered = (await exporter.plan(formats, job.quality)).canSaveCaptions;
        } catch {
          return get().reset();
        }
        await get().start(formats, job.quality, {
          captionFile: chosenCaptionFile(formats, offered),
          replaces: job.jobId,
        });
      } finally {
        set({ startingOver: false });
      }
    },
    loadJobs: async () => {
      const id = ++listing;
      try {
        const jobs = await exporter.jobs();
        if (id === listing) set({ jobs });
      } catch {
        // The list stays as it was.
      }
    },
    onEvent: (e) => {
      set((s) => applyEvent(s, e));
      if (!ENDS.has(e.type)) return;
      const s = get();
      if (s.ready.length > 0) void readWritten(batch);
      // The job is kept, or gone: the list says so.
      void s.loadJobs();
      // With the dialog open it says so itself; closed, a toast does.
      if (s.open) return;
      if (e.type === 'finished')
        toast.info(exportedText(s), {
          done: true,
          actions: [{ label: 'Show in Finder', run: () => void revealExported() }],
        });
      else if (s.phase === 'failed') toast.error(s.announcement);
      else if (s.phase === 'paused') toast.info(s.announcement);
    },
    reset: () => {
      batch++;
      set({ ...idleRun, projectId: null, written: [], replaces: null });
      // The job that was on screen joins the unfinished list as it is now.
      void get().loadJobs();
    },
  };
});

/**
 * Another project opened (or none): a run that ended without a job (it failed or stopped before starting) was the last
 * project's and goes.
 */
useProject.subscribe((s, prev) => {
  const id = s.snapshot?.project.id ?? null;
  if (id === (prev.snapshot?.project.id ?? null)) return;
  const run = useExport.getState();
  if (!run.jobId && run.phase !== 'idle' && run.phase !== 'running' && run.projectId !== id) run.reset();
});
