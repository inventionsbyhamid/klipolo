import { plural, t } from '@lingui/core/macro';
import type { CodedError } from '../bindings/CodedError';
import type { ExportEvent } from '../bindings/ExportEvent';
import type { ExportJobInfo } from '../bindings/ExportJobInfo';
import type { FormatKind } from '../bindings/FormatKind';
import type { PlannedFile } from '../bindings/PlannedFile';
import type { Said } from '../bindings/Said';
import { codedMessage } from '../bridge/call';
import { FORMAT_SHORT } from '../lib/layout';

export type ExportPhase = 'idle' | 'running' | 'paused' | 'done' | 'failed' | 'cancelled';
/** Why a finished export has no captions file beside its video: a full disk, another failure, or no words in it. */
export type CaptionFailure = 'diskFull' | 'failed' | 'empty';
/** A captions file in place beside a video: its `fileReady` index (past the videos') and its name. */
export interface CaptionFileReady {
  index: number;
  name: string;
}

/**
 * One export run as the dialog shows it. An export that stops before every file is in place keeps its job and every
 * finished part: paused (Pause, Quit, a full disk, a busy encoder) or failed, it resumes from there, and the files
 * already in place stay listed.
 */
export interface ExportRun {
  phase: ExportPhase;
  /** The run's job (from `started`, or the one resumed): Cancel export, Resume, Try again and Discard act on it. */
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
  /** Captions files in place beside the videos. */
  captionFiles: CaptionFileReady[];
  /** The video is in place but its captions file isn't: why. */
  captionFailed: CaptionFailure | null;
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
  captionFiles: [],
  captionFailed: null,
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

/** Why the captions file isn't beside the video, plainly, with what to do. */
export function captionFailure(why: CaptionFailure): string {
  if (why === 'empty')
    return t({
      id: 'export.captionFile.empty',
      message: 'There were no captions to save: none of the transcribed words are in your video.',
      comment: 'A captions file was asked for, but the words transcribed were all cut from the video',
    });
  return why === 'diskFull'
    ? t({
        id: 'export.captionFile.diskFull',
        message:
          'The captions file couldn’t be saved because the disk is full. Your video is ready. Free up space, then export again to save it.',
      })
    : t({
        id: 'export.captionFile.failed',
        message: 'The captions file couldn’t be saved. Your video is ready. Export again to save it.',
      });
}

/** "Exported 1 video.", and why its captions file isn't beside it when it isn't. */
export function exportedText(s: Pick<ExportRun, 'files' | 'captionFailed'>): string {
  const title = `${exportedTitle(s.files.length)}.`;
  return s.captionFailed ? `${title} ${captionFailure(s.captionFailed)}` : title;
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

/** A failure in words: which formats were not saved first, then why, which ends with what to do. */
export function failedText(why: string, lost: FormatKind[]): string {
  return [notSaved(lost), why].filter(Boolean).join(' ');
}

/** A run that failed: why, and which of its formats were not saved. */
function failed(s: ExportRun, why: string): Partial<ExportRun> {
  return {
    ...stopped,
    phase: 'failed',
    message: why,
    announcement: failedText(why, unsavedFormats(s)),
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
    case 'progress': {
      // Said at each quarter (progress is announced at 25 % steps, not every frame).
      const was = Math.floor(exportPercent(s) / 25);
      const now = Math.floor(exportPercent(e) / 25);
      const quarter =
        now > was && now < 4
          ? { announcement: t({ id: 'export.announce.quarter', message: `${now * 25}% exported.` }) }
          : {};
      return {
        ...quarter,
        frame: e.frame,
        totalFrames: e.totalFrames,
        segmentsDone: e.segmentsDone,
        segmentsTotal: e.segmentsTotal,
        etaS: s.finishing ? null : e.etaS,
        // "Trying again" until the new attempt has measured its time left.
        retrying: s.retrying && e.etaS === null,
      };
    }
    case 'finishing':
      return { finishing: true, etaS: null };
    case 'retrying':
      return { retries: s.retries + 1, retrying: true, finishing: false, etaS: null };
    case 'fileReady':
      // Past the videos: the captions file beside one of them.
      if (e.index >= s.files.length)
        return {
          captionFiles: [
            ...s.captionFiles.filter((c) => c.index !== e.index),
            { index: e.index, name: fileName(e.path) },
          ],
        };
      // A name taken since planning gets the next free one: the row shows the file as it is on disk.
      return {
        files: s.files.map((f, i) => (i === e.index ? { ...f, path: e.path, name: fileName(e.path) } : f)),
        ready: s.ready.includes(e.index) ? s.ready : [...s.ready, e.index],
      };
    case 'captionFileFailed':
      return { captionFailed: e.diskFull ? 'diskFull' : 'failed' };
    case 'captionFileEmpty':
      return { captionFailed: 'empty' };
    case 'finished':
      // Why there's no captions file is heard with the title, not only shown.
      return {
        ...stopped,
        phase: 'done',
        announcement: s.captionFailed ? exportedText(s) : exportedTitle(s.files.length),
      };
    case 'paused': {
      // Paused by the person (Pause, Quit), or why: a full disk, a busy encoder. Calm, not a failure: it resumes.
      const why = e.message === null ? null : saidText(e.message);
      return {
        ...stopped,
        phase: 'paused',
        message: why,
        announcement: why
          ? t({ id: 'export.announce.pausedWhy', message: `Export paused. ${why}` })
          : t({ id: 'export.announce.paused', message: 'Export paused.' }),
      };
    }
    case 'failed':
      return failed(s, saidText(e.message));
    case 'cancelled':
      return {
        ...stopped,
        phase: 'cancelled',
        jobId: null,
        announcement: t({ id: 'export.announce.stopped', message: 'Export stopped. Nothing was saved.' }),
      };
    // Notes and the screens' analysis aren't shown in this dialog.
    default:
      return {};
  }
}

/**
 * The run shown while job `jobId` resumes, until its `started` arrives: what the job list says (`job`), or what was on
 * screen (`s`, when it showed that job).
 */
export function resuming(s: ExportRun, jobId: string, job?: ExportJobInfo): Partial<ExportRun> {
  const shown = s.jobId === jobId ? s : idleRun;
  return {
    ...idleRun,
    phase: 'running',
    jobId,
    files: job?.files ?? shown.files,
    // The files already in place stay listed.
    ready: shown.ready,
    captionFiles: shown.captionFiles,
    frame: job?.frame ?? shown.frame,
    totalFrames: job?.totalFrames ?? shown.totalFrames,
    segmentsDone: job?.segmentsDone ?? shown.segmentsDone,
    segmentsTotal: job?.segmentsTotal ?? shown.segmentsTotal,
    announcement: t({ id: 'export.announce.resumed', message: 'Export resumed.' }),
  };
}

/** What the dialog reads to show the run and the unfinished jobs: the run on screen and the jobs Rust listed. */
type JobsView = Pick<ExportRun, 'jobId'> & { jobs: ExportJobInfo[] };

/** The job on screen, as `export_jobs` last listed it. */
export function shownJob(s: JobsView): ExportJobInfo | undefined {
  return s.jobId ? s.jobs.find((j) => j.jobId === s.jobId) : undefined;
}

/** Unfinished exports other than the one on screen. */
export function unfinishedJobs(s: JobsView): ExportJobInfo[] {
  return s.jobs.filter((j) => j.jobId !== s.jobId);
}

/** Whether any of `job`'s formats is still in the project, so Start over has something to export. */
export const canStartOver = (job: ExportJobInfo, inProject: readonly FormatKind[]) =>
  job.files.some((f) => inProject.includes(f.format));

/** Rust's code for a refusal (`exports.refused.*`), from the error the bridge threw; '' for any other. */
function refusalCode(error: unknown): { code: string; params: CodedError['params'] } {
  const cause: unknown = error instanceof Error ? error.cause : undefined;
  if (typeof cause === 'object' && cause !== null && typeof (cause as CodedError).code === 'string')
    return cause as CodedError;
  return { code: '', params: {} };
}

/** Why Rust didn't pause an export (`error` from the bridge), in words, by Rust's code; else a generic one. */
export function pauseRefusal(error: unknown): string {
  const { code } = refusalCode(error);
  if (code === 'exports.refused.notRunning')
    return t({ id: 'export.pauseRefused.notRunning', message: 'The export didn’t pause. It isn’t running any more.' });
  return t({ id: 'export.pauseRefused.other', message: 'The export didn’t pause. Try again in a moment.' });
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
