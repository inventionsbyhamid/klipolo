import { create } from 'zustand';
import type { ExportedFile } from '../bindings/ExportedFile';
import type { ExportEvent } from '../bindings/ExportEvent';
import type { ExportQuality } from '../bindings/ExportQuality';
import type { FormatKind } from '../bindings/FormatKind';
import { exporter } from '../bridge/exporter';
import { errorMessage } from '../lib/errors';
import { flushBursts } from './bursts';
import { commitsDone } from './commitQueue';
import { applyEvent, exportedTitle, idleRun, refusalReason, unsavedFormats, type ExportRun } from './exportRun';
import { settled, useProject } from './project';
import { toast } from './toasts';

export { exportPercent, type ExportPhase } from './exportRun';

interface ExportStore extends ExportRun {
  /** The export dialog is on screen. */
  open: boolean;
  /** The quality the dialog has picked: High unless the person picks another. */
  quality: ExportQuality;
  /** The formats the person checked in the dialog; null: the format the preview shows, until they pick. */
  picked: FormatKind[] | null;
  /** What the running (or last) export asked for: Try again asks the same, less what is in place. */
  formats: FormatKind[];
  runQuality: ExportQuality;
  /** The project the run on screen exports: Try again never exports another one. */
  projectId: string | null;
  /** Where the files go, as the dialog names it ("Movies / Klipolo"), once a plan or Change has said. */
  folder: string | null;
  /** The finished files with their sizes on the disk, once the run has ended with some in place. */
  written: ExportedFile[];
  setOpen: (open: boolean) => void;
  setQuality: (quality: ExportQuality) => void;
  setPicked: (formats: FormatKind[]) => void;
  setFolder: (folder: string) => void;
  /** Asks for the folder the files go to (Change); true when one was picked. */
  pickFolder: () => Promise<boolean>;
  /** Exports `formats` of the open project, every edit already made included. */
  start: (formats: FormatKind[], quality?: ExportQuality) => Promise<void>;
  /** The formats a failed run didn't save, again (a run that never got a job: all of them). */
  retry: () => Promise<void>;
  /** Stops the run on screen. Resolves on Rust's answer: null, or why not (worded). */
  cancel: () => Promise<string | null>;
  onEvent: (event: ExportEvent) => void;
  reset: () => void;
}

/**
 * Shows the last export's first file (or file `index`) in Finder; a file that is gone says so in a toast (Rust's
 * `exports.revealGone`, worded by the bridge).
 */
export function revealExported(index?: number): Promise<void> {
  const shown = index === undefined ? exporter.reveal() : exporter.reveal(index);
  return shown.catch((error: unknown) => toast.error(errorMessage(error)));
}

/** Opens file `index` of the last export in its app; one that is gone says so in a toast. */
export function openExported(index: number): Promise<void> {
  return exporter.open(index).catch((error: unknown) => toast.error(errorMessage(error)));
}

/** Events of an earlier run are ignored. */
let batch = 0;

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
    formats: [],
    runQuality: 'high',
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
        return;
      }
      set({ open });
      // A run that ended is put away with the dialog; one still running carries on (the top bar shows it).
      const { phase } = get();
      if (phase === 'done' || phase === 'failed' || phase === 'cancelled') get().reset();
    },
    setQuality: (quality) => set({ quality }),
    setPicked: (picked) => set({ picked }),
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
    start: async (formats, quality = get().quality) => {
      const id = ++batch;
      set({
        ...idleRun,
        phase: 'running',
        formats,
        runQuality: quality,
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
        await exporter.start(formats, quality, listener(id));
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
      const { files, ready, formats, projectId, runQuality } = get();
      // A run that never got a job is its project's: in another project it is gone, not exported again.
      if (projectId !== openProjectId()) {
        get().reset();
        return Promise.resolve();
      }
      // The files already in place stay: only what wasn't saved is made again.
      const again = files.length > 0 ? unsavedFormats({ files, ready }) : formats;
      return get().start(again.length > 0 ? again : formats, runQuality);
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
    onEvent: (e) => {
      set((s) => applyEvent(s, e));
      if (!ENDS.has(e.type)) return;
      const s = get();
      if (s.ready.length > 0) void readWritten(batch);
      // With the dialog open it says so itself; closed, a toast does.
      if (s.open) return;
      if (e.type === 'finished')
        toast.info(`${exportedTitle(s.files.length)}.`, {
          done: true,
          actions: [{ label: 'Show in Finder', run: () => void revealExported() }],
        });
      else if (s.phase === 'failed') toast.error(s.announcement);
    },
    reset: () => {
      batch++;
      set({ ...idleRun, projectId: null, written: [] });
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
