import { t } from '@lingui/core/macro';
import type { CodedError } from '../bindings/CodedError';
import type { MediaJobInfo } from '../bindings/MediaJobInfo';
import type { ProxyReason } from '../bindings/ProxyReason';
import { codedMessage } from '../bridge/call';

/*
 * The one place that reads and words Rust's reasons for editing copies: codes of the app's table (`copies.*`, worded
 * from `appCodes.json`, which Rust writes) and `ProxyReason`, never English sent by Rust. The page's own sentences
 * (who paused, why a file gets a copy) are `mediaJobs.*`.
 */

/**
 * What a row says (`media_jobs::schedule`, `Job::reason`; a waiting row has no reason): waiting its turn, paused while
 * the video plays, an export runs or a take records, paused by a hold (the person's, an action's, or automatic copies
 * off: the list's `held` says which), paused for a full disk, heat or a busy encoder, its worker stopped and about to
 * start again (`workerStopped`), the file missing, or a copy that couldn't be made.
 */
export type CopyWhy =
  | 'waiting'
  | 'playing'
  | 'exporting'
  | 'recording'
  | 'held'
  | 'diskFull'
  | 'hot'
  | 'encoderBusy'
  | 'workerStopped'
  | 'missing'
  | 'failed';

/**
 * Why Rust didn't take a command (`copies.refused.*`): the file isn't the open project's, the copies' driver is gone
 * (`unavailable`: nothing was applied), or it answered late (`slowDrive`: the command is still applied once the drive
 * answers); `other` for anything else (no Tauri, the IPC closed).
 */
export type RefusalKind = 'notInProject' | 'unavailable' | 'slowDrive' | 'other';

/** A missing source's code: a copy's row and the preview's notice share it (Rust's `MEDIA_MISSING`). */
export const MISSING_CODE = 'copies.failed.missing';

const WHY: Record<string, CopyWhy> = {
  'copies.paused.playing': 'playing',
  'copies.paused.exporting': 'exporting',
  'copies.paused.recording': 'recording',
  'copies.paused.held': 'held',
  'copies.paused.diskFull': 'diskFull',
  'copies.paused.hot': 'hot',
  'copies.paused.encoderBusy': 'encoderBusy',
  'copies.paused.workerStopped': 'workerStopped',
  [MISSING_CODE]: 'missing',
};
/** Every other failure (unreadable, unknown length, a read-only disk, a busy encoder) is a copy that couldn't be made. */
const FAILED = 'copies.failed.';

const REFUSED: Record<string, RefusalKind> = {
  'copies.refused.notInProject': 'notInProject',
  'copies.refused.unavailable': 'unavailable',
  'copies.refused.slowDrive': 'slowDrive',
};

/** What a row's reason says (a waiting row: waiting), or null for a code this build doesn't know. */
export function copyWhy(job: MediaJobInfo): CopyWhy | null {
  if (job.state === 'waiting') return 'waiting';
  const code = job.reason?.code ?? '';
  if (Object.hasOwn(WHY, code)) return WHY[code];
  if (code.startsWith(FAILED)) return 'failed';
  return null;
}

/** Which refusal a command's error is (`call` keeps Rust's coded error as the error's cause). */
export function refusalKind(error: unknown): RefusalKind {
  const cause: unknown = error instanceof Error ? error.cause : undefined;
  const code =
    typeof cause === 'object' && cause !== null && typeof (cause as { code?: unknown }).code === 'string'
      ? (cause as { code: string }).code
      : '';
  return Object.hasOwn(REFUSED, code) ? REFUSED[code] : 'other';
}

/** A refusal in plain words (a slow drive's command is still applied, so it is never said). */
export function refusalText(kind: Exclude<RefusalKind, 'slowDrive'>): string {
  switch (kind) {
    case 'notInProject':
      return codedMessage({ code: 'copies.refused.notInProject', params: {} });
    case 'unavailable':
      return codedMessage({ code: 'copies.refused.unavailable', params: {} });
    case 'other':
      return t({ id: 'mediaJobs.refused.other', message: "That didn't work. Try again in a moment." });
  }
}

/** Paused by a hold (`copies.paused.held`); also how a paused row with a code this build doesn't know is said. */
export function pausedText(): string {
  return codedMessage({ code: 'copies.paused.held', params: {} });
}

/** A copy that couldn't be made (`copies.failed.other`); also how a failure this build doesn't know is said. */
export function failedText(name: string): string {
  return codedMessage({ code: 'copies.failed.other', params: { name } });
}

/** A text value of a reason (`name`), else `fallback`. */
function param(reason: CodedError, key: string, fallback: string): string {
  const value = reason.params[key];
  return typeof value === 'string' ? value : fallback;
}

/**
 * A row's reason in plain words, from Rust's table; null for no reason, or a code this build doesn't know (the row is
 * then said by its state). `name` (the file) comes with the code, else the row's.
 */
export function reasonText(job: MediaJobInfo): string | null {
  const { reason } = job;
  if (!reason) return null;
  const said = codedMessage({ ...reason, params: { ...reason.params, name: param(reason, 'name', job.name) } });
  return said === '' ? null : said;
}

/** Why a file gets an editing copy (`needs_proxy`), shown under its name while its copy is to be made. */
export function becauseText(why: ProxyReason): string {
  switch (why) {
    case 'noHardwareDecode':
      return t({
        id: 'mediaJobs.because.noHardwareDecode',
        message: "This Mac can't decode its format in hardware.",
        comment: 'Why Klipolo makes an editing copy of this file',
      });
    case 'tenBit':
      return t({
        id: 'mediaJobs.because.tenBit',
        message: 'Its 10-bit color is slow to decode on this Mac.',
        comment: 'Why Klipolo makes an editing copy of this file',
      });
    case 'hdr':
      return t({
        id: 'mediaJobs.because.hdr',
        message: 'Its HDR video is slow to decode on this Mac.',
        comment: 'Why Klipolo makes an editing copy of this file',
      });
    case 'tooLarge':
      return t({
        id: 'mediaJobs.because.tooLarge',
        message: "It's larger than this Mac plays smoothly.",
        comment: 'Why Klipolo makes an editing copy of this file (a 4K video on a smaller GPU)',
      });
    case 'longGop':
      return t({
        id: 'mediaJobs.because.longGop',
        message: 'Its keyframes are far apart, so it plays and scrubs slowly.',
        comment: 'Why Klipolo makes an editing copy of this file',
      });
    case 'variableRate':
      return t({
        id: 'mediaJobs.because.variableRate',
        message: 'Its frame rate changes, so it scrubs unevenly.',
        comment: 'Why Klipolo makes an editing copy of this file',
      });
  }
}
