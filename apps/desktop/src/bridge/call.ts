import { i18n } from '@lingui/core';
import { t } from '@lingui/core/macro';
import { invoke } from '@tauri-apps/api/core';
import type { CodedError } from '../bindings/CodedError';
import type { Said } from '../bindings/Said';
import { wordParams } from '../lib/labels';
import { RECORDER_WORDS } from '../lib/recorder/words';

function isCoded(error: unknown): error is CodedError {
  return (
    typeof error === 'object' && error !== null && typeof (error as { code?: unknown }).code === 'string'
  );
}

/** The app's own codes (`crates/klipolo-app`: the project and the preview commands), with their English. */
const APP_CODES: Record<string, string> = {
  'engine.play.rateUnavailable': "That playback speed isn't available.",
  'engine.play.noProject': 'Open a project to play it.',
  'engine.play.noVideo': 'Add a video to play it.',
  'engine.stopped': 'The preview has stopped.',
  'engine.notReady': "The preview isn't ready yet.",
  'engine.audioDamaged':
    "The sound of {file} can't be read after {atUs}, so it's silent from there. The file may be damaged.",
  'engine.waitingForDisk': "Waiting for the disk with {name}. Check that it's connected and awake.",
  'exports.noFormat': 'Pick at least one format.',
  'exports.noProject': 'No project is open.',
  'exports.noVideosFolder': "Klipolo couldn't find your Movies folder.",
  'exports.empty': 'Add a video before exporting.',
  'exports.unsaved': "Your latest changes couldn't be saved, so the export didn't start.",
  'exports.notStarted': "The export didn't start. Try again.",
  'exports.folder.diskFull': "The export didn't start. The disk is full. Free up space, then try again.",
  'exports.folder.readOnly': "The export didn't start. {folder} is on a disk that can't be written to.",
  'exports.folder.denied': "The export didn't start. Klipolo isn't allowed to write to {folder}.",
  'exports.folder.failed': "The export didn't start. Klipolo couldn't make the folder {folder}.",
  'exports.fontsMissing': "Some fonts aren't on this Mac ({names}). Pick another font, then export.",
  'exports.refused.busy': 'An export is already running.',
  'exports.refused.notRunning': "That export isn't running.",
  'exports.refused.otherProject': 'The running export is of {project}, not this project.',
  'exports.refused.invalid': "That export isn't valid.",
  'exports.unreadable': "This export's files can't be read. Discard it and export again.",
  'exports.discardFailed': "The export couldn't be discarded. Try again.",
  'exports.gone': "That export isn't there any more. Export again.",
  'exports.otherBuild': 'Another version of Klipolo made this export, so it can’t resume. Start over or discard it.',
  'exports.revealGone': "That video isn't there any more.",
  'exports.shareFailed': "Klipolo couldn't open the share menu. Show the videos in Finder and share them from there.",
  'exports.diskFull': 'The disk is full. Free up space, then resume.',
  'exports.encoderBusy':
    'The video encoder is busy. Another app may be using it, like a screen recorder or a video call. Close it, then resume.',
  'exports.decodeFailed':
    "A clip couldn't be decoded with hardware. Resume to try again, or finish in compatibility mode.",
  'exports.gpuFailed': 'The graphics processor stopped responding. Resume to try again, or finish in compatibility mode.',
  'exports.stopped': 'The export stopped unexpectedly. Resume to try again, or finish in compatibility mode.',
  'exports.frame.fontMissing': "A font used at {atUs} isn't on this Mac. Pick another font, then export.",
  'exports.frame.fontMissingNamed': "A font used at {atUs} isn't on this Mac ({names}). Pick another font, then export.",
  'exports.frame.cjkFontMissing': 'Chinese, Japanese or Korean text at {atUs} needs a font Klipolo doesn’t have yet.',
  'exports.frame.captionStyleMissing':
    "The captions at {atUs} use a style that isn't here ({style}). Pick another caption style, then export.",
  'exports.frame.tooManyTexts': 'Too many large texts at {atUs} to export. Make some smaller or remove some.',
  'exports.frame.tooManyEffects': 'Too many layers with effects at {atUs} to export. Turn some effects off there.',
  'exports.frame.other': '{reason} (at {atUs})',
  'project.none': 'No project is open.',
  'project.moved': 'The video changed. Try again.',
  'project.createFailed': "Klipolo couldn't create the project. Check the disk, then try again.",
  'project.noVideosFolder': "Klipolo couldn't find your Movies folder.",
  'project.libraryFailed': "Klipolo couldn't make its projects folder. Check the disk, then try again.",
  'project.revealGone': "This project's file isn't there any more.",
  'project.revealFailed': "Klipolo couldn't open Finder. Try again.",
};

/**
 * Every code list Rust exports (`modelCodes.json`, `fontsCodes.json`, and the app's `appCodes.json`: editing copies,
 * and the preview's notice for a file it needs that isn't there), by code: committed, and written again by
 * `pnpm bindings`.
 */
const RUST_CODES = new Map(
  Object.values(
    import.meta.glob<{ code: string; message: string }[]>('../bindings/*Codes.json', {
      eager: true,
      import: 'default',
    }),
  )
    .flat()
    .map((entry) => [entry.code, entry.message]),
);

/**
 * A coded error or notice from Rust (Rust sends codes, never English) in English; '' for an unknown code. Its values
 * are worded as labels' are (`wordParams`: a time as a clock, a nested code by its own words). A recorder note that
 * would offer what Klipolo doesn't have is said in Klipolo's words (`RECORDER_WORDS`).
 */
export function codedMessage(error: CodedError): string {
  const message = APP_CODES[error.code] ?? RECORDER_WORDS[error.code] ?? RUST_CODES.get(error.code);
  return message === undefined ? '' : i18n._(error.code, wordParams(error.params), { message });
}

/** Whether `error` (from `call`) carries the Rust code `code`. */
export function isCode(error: unknown, code: string): boolean {
  const cause = error instanceof Error ? error.cause : undefined;
  return isCoded(cause) && cause.code === code;
}

/**
 * What `error` (from `call`) says, kept as Rust sent it when it is a code (chat keeps its replies so); else its words.
 */
export function saidOf(error: unknown): Said {
  const cause = error instanceof Error ? error.cause : undefined;
  if (isCoded(cause)) return cause;
  return error instanceof Error ? error.message : String(error);
}

/** What Tauri says when the app has no command by that name (a page newer than the app it runs in). */
const NO_COMMAND = /^Command \S+ not found$/;

/**
 * Invoke a Tauri command, turning Rust's errors into Errors with a plain message. A coded error (`{ code, params }`) is
 * worded here; the rest are already sentences, except Tauri's own for a command the app doesn't have. The error keeps
 * what Rust (or Tauri) sent as its `cause` (`isCode`, `saidOf`).
 */
export async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    const message =
      typeof error === 'string' && NO_COMMAND.test(error)
        ? t({ id: 'error.noCommand', message: "Klipolo can't do that in this version. Restart Klipolo and try again." })
        : typeof error === 'string'
          ? error
          : isCoded(error)
            ? codedMessage(error)
            : error instanceof Error
              ? error.message
              : '';
    throw new Error(message || t({ id: 'error.unknown', message: 'Something went wrong. Try again.' }), {
      cause: error,
    });
  }
}
