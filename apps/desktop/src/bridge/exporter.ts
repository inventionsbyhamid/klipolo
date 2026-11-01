import { Channel } from '@tauri-apps/api/core';
import type { CaptionFile } from '../bindings/CaptionFile';
import type { ExportedFile } from '../bindings/ExportedFile';
import type { ExportEvent } from '../bindings/ExportEvent';
import type { ExportJobInfo } from '../bindings/ExportJobInfo';
import type { ExportPlan } from '../bindings/ExportPlan';
import type { ExportQuality } from '../bindings/ExportQuality';
import type { FormatKind } from '../bindings/FormatKind';
import type { ResumeMode } from '../bindings/ResumeMode';
import { call } from './call';

/** A box in the page, in CSS px from its top left (an element's `getBoundingClientRect`). */
export interface PageRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

function channel(onEvent: (event: ExportEvent) => void): Channel<ExportEvent> {
  const events = new Channel<ExportEvent>();
  events.onmessage = onEvent;
  return events;
}

/** The export commands (`crates/klipolo-app/src/commands/export.rs`). */
export const exporter = {
  /** The files an export would write, with its size and time estimate and the folder they go to. */
  plan: (formats: FormatKind[], quality: ExportQuality) =>
    call<ExportPlan>('export_plan', { req: { formats, quality } }),
  /**
   * Resolves once the export has started; its progress arrives on `onEvent`. `options.captionFile`: also save the
   * captions as that kind of file beside the 16:9 video. `options.replaces`: Start over, the unfinished job this export
   * replaces, which Rust discards only once the new export holds the slot and is ready to run (a refused start keeps
   * it).
   */
  start: (
    formats: FormatKind[],
    quality: ExportQuality,
    onEvent: (event: ExportEvent) => void,
    options?: { captionFile?: CaptionFile; replaces?: string },
  ) =>
    call<void>('export_start', {
      req: {
        formats,
        quality,
        ...(options?.captionFile ? { captionFile: options.captionFile } : {}),
        ...(options?.replaces ? { replaces: options.replaces } : {}),
      },
      events: channel(onEvent),
    }),
  /** Unfinished exports on disk, newest first: running, paused, stopped by a quit or a crash, or failed. */
  jobs: () => call<ExportJobInfo[]>('export_jobs'),
  /** Pauses a running export at its next part, keeping every finished part. */
  pause: (job: string) => call<void>('export_pause', { job }),
  /**
   * Continues an unfinished export from its finished parts (`compatible`: in compatibility mode); its progress arrives
   * on `onEvent`, as for `start`.
   */
  resume: (job: string, onEvent: (event: ExportEvent) => void, mode?: ResumeMode) =>
    call<void>('export_resume', { job, events: channel(onEvent), ...(mode ? { mode } : {}) }),
  /** Deletes an unfinished export and its finished parts (a running one is stopped first). */
  discard: (job: string) => call<void>('export_discard', { job }),
  /**
   * Stops the running export and deletes its job and parts. `job` names it (the run on screen); without it only the
   * open project's export is stopped (another project's is refused by name).
   */
  cancel: (job?: string) => call<void>('export_cancel', job ? { job } : {}),
  /** Shows file `index` of the last export in Finder; without one, its folder with the first file selected. */
  reveal: (index?: number) => call<void>('export_reveal', index === undefined ? {} : { index }),
  /**
   * Shows the Mac's share menu (AirDrop, Messages, Mail and the like) for the last export's files, beside `anchor` (the
   * button pressed, in page px). Resolves once the menu shows.
   */
  share: (anchor: PageRect) => call<void>('export_share', { anchor }),
  /** Opens file `index` of the last export in its app. */
  open: (index: number) => call<void>('export_open', { index }),
  /** The last export's files in place, by index, with their sizes. */
  files: () => call<ExportedFile[]>('export_files'),
  /** Asks for the folder exports go to: its name ("Desktop / Clips"), or null when the person cancels. */
  pickFolder: () => call<string | null>('export_pick_folder'),
};
