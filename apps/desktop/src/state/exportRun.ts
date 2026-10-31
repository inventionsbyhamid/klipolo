import { plural, t } from '@lingui/core/macro';
import type { CodedError } from '../bindings/CodedError';
import type { ExportEvent } from '../bindings/ExportEvent';
import type { FormatKind } from '../bindings/FormatKind';
import type { PlannedFile } from '../bindings/PlannedFile';
import type { Said } from '../bindings/Said';
import { codedMessage } from '../bridge/call';
import { FORMAT_SHORT } from '../lib/layout';

export type ExportPhase = 'idle' | 'running' | 'done' | 'failed' | 'cancelled';

/**
 * One export run as the dialog shows it. Klipolo doesn't resume an export: one that stops before every file is in place
 * has failed, its job and parts deleted, and the files already in place stay listed.
 */
export interface ExportRun {
  phase: ExportPhase;
  /** The run's job (from `started`): Cancel export names it. */
  jobId: string | null;
  files: PlannedFile[];
  /** Every format's frames done and to do (two formats of a 900-frame video: 1800). */
  frame: number;
  totalFrames: number;
  /** Finished, kept parts (about 30 s of one format each). */
  segmentsDone: number;
  segmentsTotal: number;
  /** Measured time left, once this run has rendered for 5 s. */
  etaS: number | null;
  /** Joining the finished parts into the files: no time left to show. */
  finishing: boolean;
  /** Retries in this run: the runner tries hardware, hardware again, then compatibility mode. */
  retries: number;
  /** A retry that hasn't measured its own time left yet. */
  retrying: boolean;
  /** Cancel export was pressed. */
  stopping: boolean;
  /** Indexes of files that are in place. */
  ready: number[];
  /** Why it failed, in words. */
  message: string | null;
  /** Saving the latest edits before the export takes a moment. */
  saving: boolean;
  /** What the dialog's status region says: done, failed, stopped. */
  announcement: string;
}

export const idleRun: ExportRun = {
  phase: 'idle',
  jobId: null,
  files: [],
  frame: 0,
  totalFrames: 0,
  segmentsDone: 0,
  segmentsTotal: 0,
  etaS: null,
  finishing: false,
  retries: 0,
  retrying: false,
  stopping: false,
  ready: [],
  message: null,
  saving: false,
  announcement: '',
};

/** Every way a run stops clears what was in flight. */
const stopped = {
  etaS: null,
  finishing: false,
  retrying: false,
  stopping: false,
};

function fileName(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1);
}

/** What Rust said, in words: a code worded by the app's tables, or its own sentence. */
export function saidText(said: Said): string {
  if (typeof said === 'string') return said;
  return codedMessage(said as CodedError) || t({ id: 'export.failed.other', message: 'The export stopped.' });
}

/** "Exported 2 videos". */
export function exportedTitle(count: number): string {
  return t({
    id: 'export.done.title',
    message: plural(count, { one: 'Exported # video', other: 'Exported # videos' }),
  });
}

/** The formats of the run's files that aren't in place. */
export function unsavedFormats(s: Pick<ExportRun, 'files' | 'ready'>): FormatKind[] {
  return s.files.filter((_, i) => !s.ready.includes(i)).map((f) => f.format);
}

/** "9:16 was not saved.", "9:16 and 1:1 were not saved."; '' when every file is in place. */
export function notSaved(formats: FormatKind[]): string {
  if (formats.length === 0) return '';
  const names = formats.map((f) => FORMAT_SHORT[f]);
  const list = names.length > 1 ? `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}` : names[0];
  return formats.length > 1
    ? t({ id: 'export.failed.notSavedMany', message: `${list} were not saved.` })
    : t({ id: 'export.failed.notSavedOne', message: `${list} was not saved.` });
}

/** A run that failed: why, and which of its formats were not saved. */
function failed(s: ExportRun, why: string): Partial<ExportRun> {
  const lost = notSaved(unsavedFormats(s));
  return {
    ...stopped,
    phase: 'failed',
    message: why,
    announcement: lost ? `${why} ${lost}` : why,
  };
}

/** What `event` changes in the run. */
export function applyEvent(s: ExportRun, e: ExportEvent): Partial<ExportRun> {
  switch (e.type) {
    case 'started':
      return {
        jobId: e.jobId,
        files: e.files,
        totalFrames: e.totalFrames,
        segmentsTotal: e.segmentsTotal,
      };
    case 'progress':
      return {
        frame: e.frame,
        totalFrames: e.totalFrames,
        segmentsDone: e.segmentsDone,
        segmentsTotal: e.segmentsTotal,
        etaS: s.finishing ? null : e.etaS,
        // "Trying again" until the new attempt has measured its time left.
        retrying: s.retrying && e.etaS === null,
      };
    case 'finishing':
      return { finishing: true, etaS: null };
    case 'retrying':
      return { retries: s.retries + 1, retrying: true, finishing: false, etaS: null };
    case 'fileReady':
      // Past the videos: a caption file beside one of them (Klipolo asks for none).
      if (e.index >= s.files.length) return {};
      // A name taken since planning gets the next free one: the row shows the file as it is on disk.
      return {
        files: s.files.map((f, i) => (i === e.index ? { ...f, path: e.path, name: fileName(e.path) } : f)),
        ready: s.ready.includes(e.index) ? s.ready : [...s.ready, e.index],
      };
    case 'finished':
      return { ...stopped, phase: 'done', announcement: exportedTitle(s.files.length) };
    case 'paused':
      // Rust says a run that stopped keeping its parts as a failure; one that still arrives as a pause is one too.
      return failed(s, e.message ?? t({ id: 'export.failed.stopped', message: 'The export stopped.' }));
    case 'failed':
      return failed(s, saidText(e.message));
    case 'cancelled':
      return {
        ...stopped,
        phase: 'cancelled',
        jobId: null,
        announcement: t({ id: 'export.announce.stopped', message: 'Export stopped. Nothing was saved.' }),
      };
    // Notes, the screens' analysis and caption files are about recordings and caption files Klipolo doesn't make.
    default:
      return {};
  }
}

/** Rust's code for a refusal (`exports.refused.*`), from the error the bridge threw; '' for any other. */
function refusalCode(error: unknown): { code: string; params: CodedError['params'] } {
  const cause: unknown = error instanceof Error ? error.cause : undefined;
  if (typeof cause === 'object' && cause !== null && typeof (cause as CodedError).code === 'string')
    return cause as CodedError;
  return { code: '', params: {} };
}

/**
 * Why Rust didn't stop the export (`error` from the bridge), in words, by Rust's code; else a generic one.
 */
export function refusalReason(error: unknown): string {
  const { code, params } = refusalCode(error);
  if (code === 'exports.refused.notRunning')
    return t({ id: 'export.refused.notRunning', message: 'The export didn’t stop. It isn’t running any more.' });
  if (code === 'exports.refused.otherProject') {
    const project = String(params.project ?? '');
    return t({
      id: 'export.refused.otherProject',
      message: `The export didn’t stop. It’s exporting “${project}”, another project.`,
      comment: '{project} is the name of the project whose export runs.',
    });
  }
  return t({ id: 'export.refused.other', message: 'The export didn’t stop. Try again in a moment.' });
}

/** The one bar: every format's frames. Never 100 % before the files are in place (joining them takes a moment). */
export function exportPercent(s: { frame: number; totalFrames: number }): number {
  return s.totalFrames > 0 ? Math.min(99, Math.floor((s.frame / s.totalFrames) * 100)) : 0;
}
