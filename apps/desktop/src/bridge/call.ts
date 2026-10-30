import { i18n } from '@lingui/core';
import { t } from '@lingui/core/macro';
import { invoke } from '@tauri-apps/api/core';
import type { CodedError } from '../bindings/CodedError';
import type { Said } from '../bindings/Said';
import { wordParams } from '../lib/labels';

function isCoded(error: unknown): error is CodedError {
  return (
    typeof error === 'object' && error !== null && typeof (error as { code?: unknown }).code === 'string'
  );
}

/** The app's own codes (`crates/klipolo-app`: the project and the preview commands), with their English. */
const APP_CODES: Record<string, string> = {
  'engine.play.rateUnavailable': 'That playback speed isn’t available.',
  'engine.play.noProject': 'Open a project to play it.',
  'engine.play.noVideo': 'Add a video to play it.',
  'engine.stopped': 'The preview has stopped.',
  'engine.notReady': 'The preview isn’t ready yet.',
  'engine.audioDamaged':
    'The sound of {file} can’t be read after {atUs}, so it’s silent from there. The file may be damaged.',
  'engine.waitingForDisk': 'Waiting for the disk with {name}. Check that it’s connected and awake.',
  'project.none': 'No project is open.',
  'project.moved': 'The video changed. Try again.',
  'project.createFailed': 'Couldn’t create the project.',
  'project.noVideosFolder': 'Klipolo couldn’t find your Movies folder.',
  'project.libraryFailed': 'Klipolo couldn’t make its projects folder. Check the disk, then try again.',
};

/**
 * Every code list Rust exports (`modelCodes.json`, `fontsCodes.json`), by code: committed, and written again by
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
 * are worded as labels' are (`wordParams`: a time as a clock, a nested code by its own words).
 */
export function codedMessage(error: CodedError): string {
  const message = APP_CODES[error.code] ?? RUST_CODES.get(error.code);
  return message === undefined ? '' : i18n._(error.code, wordParams(error.params), { message });
}

/** Whether `error` (from `call`) carries the Rust code `code`. */
export function isCode(error: unknown, code: string): boolean {
  const cause = error instanceof Error ? error.cause : undefined;
  return isCoded(cause) && cause.code === code;
}

/** What `error` (from `call`) says, kept as Rust sent it when it is a code (chat keeps its replies so); else its words. */
export function saidOf(error: unknown): Said {
  const cause = error instanceof Error ? error.cause : undefined;
  if (isCoded(cause)) return cause;
  return error instanceof Error ? error.message : String(error);
}

/**
 * Invoke a Tauri command, turning Rust's errors into Errors with a plain message. A coded error (`{ code, params }`) is
 * worded here; the rest are already sentences. The error keeps what Rust sent as its `cause` (`isCode`, `saidOf`).
 */
export async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    const message =
      typeof error === 'string'
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
