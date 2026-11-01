import { Channel } from '@tauri-apps/api/core';
import type { CaptionFile } from '../bindings/CaptionFile';
import type { ExportedFile } from '../bindings/ExportedFile';
import type { ExportEvent } from '../bindings/ExportEvent';
import type { ExportPlan } from '../bindings/ExportPlan';
import type { ExportQuality } from '../bindings/ExportQuality';
import type { FormatKind } from '../bindings/FormatKind';
import { call } from './call';

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
   * captions as that kind of file beside the 16:9 video.
   */
  start: (
    formats: FormatKind[],
    quality: ExportQuality,
    onEvent: (event: ExportEvent) => void,
    options?: { captionFile?: CaptionFile },
  ) =>
    call<void>('export_start', {
      req: { formats, quality, ...(options?.captionFile ? { captionFile: options.captionFile } : {}) },
      events: channel(onEvent),
    }),
  /**
   * Stops the running export and deletes its job and parts. `job` names it (the run on screen); without it only the
   * open project's export is stopped (another project's is refused by name).
   */
  cancel: (job?: string) => call<void>('export_cancel', job ? { job } : {}),
  /** Shows file `index` of the last export in Finder; without one, its folder with the first file selected. */
  reveal: (index?: number) => call<void>('export_reveal', index === undefined ? {} : { index }),
  /** Opens file `index` of the last export in its app. */
  open: (index: number) => call<void>('export_open', { index }),
  /** The last export's files in place, by index, with their sizes. */
  files: () => call<ExportedFile[]>('export_files'),
  /** Asks for the folder exports go to: its name ("Desktop / Clips"), or null when the person cancels. */
  pickFolder: () => call<string | null>('export_pick_folder'),
};
