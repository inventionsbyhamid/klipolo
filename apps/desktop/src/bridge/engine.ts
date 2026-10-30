import { Channel, invoke } from '@tauri-apps/api/core';
import type { EngineEvent as EngineEventBinding } from '../bindings/EngineEvent';
import type { EngineInfo as EngineInfoBinding } from '../bindings/EngineInfo';
import type { Op } from '../bindings/Op';
import type { ViewRect as ViewRectBinding } from '../bindings/ViewRect';
import type { ViewSpec as ViewSpecBinding } from '../bindings/ViewSpec';
import { call } from './call';

/**
 * The preview engine (Rust owns it, `crates/klipolo-app/src/engine.rs`). Frames reach the screen through a native
 * layer under the webview (macOS), or over the frame socket (`frameSocket.ts`) into the page's canvas. Times are
 * integer microseconds, as in Rust; the UI converts at the edge.
 */

export type FormatKind = 'landscape' | 'portrait' | 'square';
export type Presenter = 'native' | 'socket';
export type GpuClass = 'apple' | 'integrated' | 'discrete' | 'software';

/** What `engine_attach` returns: where to connect for frames, the pixel budget, and how frames are presented. */
export interface EngineInfo {
  port: number;
  token: string;
  maxPreviewPixels: number;
  adapter: string;
  presenter: Presenter;
  gpuClass: GpuClass;
}

/** The preview the engine renders: a format at a size in device pixels. */
export interface ViewSpec {
  format: FormatKind;
  width: number;
  height: number;
}

/** Where the native layer goes: the preview hole in page px, the display scale and the colour behind it (0xRRGGBB). */
export interface ViewRect {
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
  backdrop: number | null;
}

export type StateReason = 'play' | 'pause' | 'ended' | 'hidden' | 'error';

export interface CodedError {
  code: string;
  params: Record<string, unknown>;
}

/** Mirrors `klipolo_engine::api::EngineEvent` (the crash guard's events never reach the page). */
export type EngineEvent =
  | { type: 'ready'; session: number }
  | { type: 'state'; playing: boolean; atUs: number; reason: StateReason; rate: number }
  | { type: 'warning'; message: string }
  | { type: 'error'; message: string }
  | {
      type: 'shown';
      session: number;
      seq: number;
      atUs: number;
      playback: boolean;
      sentMs: number;
      shownMs: number;
      presented: number;
    }
  | { type: 'framesLate'; atUs: number }
  | { type: 'presenter'; presenter: Presenter }
  | { type: 'media'; path: string; kind: string }
  | { type: 'notice'; notice: CodedError }
  | { type: 'waiting'; notice: CodedError | null };

/** The open project's name and length. */
export interface ProjectSummary {
  name: string;
  durationUs: number;
}

/** Starts the engine if needed; its events go to `onEvent` from now on. Says how frames reach the screen. */
export function engineAttach(onEvent: (event: EngineEvent) => void): Promise<EngineInfo> {
  const events = new Channel<EngineEvent>();
  events.onmessage = onEvent;
  return invoke<EngineInfo>('engine_attach', { events });
}

/** The format and device-pixel size to render. */
export function engineSetView(view: ViewSpec): Promise<void> {
  return invoke<void>('engine_set_view', { view });
}

/** Where the native layer goes (the preview hole), or null to hide it. */
export function engineView(rect: ViewRect | null): Promise<void> {
  return invoke<void>('engine_view', { rect });
}

/** Hidden or minimised (false): the engine pauses with silence and lets its decoders go; shown again (true). */
export function engineSetVisible(visible: boolean): Promise<void> {
  return invoke<void>('engine_set_visible', { visible });
}

/** Shows the frame at `atUs`. `seq` names the seek: frames carry it, so older ones can be dropped. */
export function engineSeek(atUs: number, seq: number): Promise<void> {
  return invoke<void>('engine_seek', { atUs, seq });
}

/** Plays from `fromUs` at `rate` (1: normal speed, with sound). `seq` names the run as a seek's does: frames carry it. */
export function enginePlay(fromUs: number, rate: number, seq: number): Promise<void> {
  return invoke<void>('engine_play', { fromUs, rate, seq });
}

/** Pauses; resolves with where playback stopped. */
export function enginePause(): Promise<number> {
  return invoke<number>('engine_pause');
}

/** The open project's name and length, or null when none is open. */
export function projectSummary(): Promise<ProjectSummary | null> {
  return invoke<ProjectSummary | null>('project_summary');
}

/**
 * The GPU preview engine (Rust owns it, `crates/klipolo-app/src/engine.rs`). Frames reach the screen through a native
 * layer under the webview (macOS), or over the frame socket (`frameSocket.ts`) into the page's canvas. Times are integer
 * microseconds, as in Rust.
 */
export const engine = {
  /**
   * Starts the engine if needed; the other calls do nothing until it runs. Says how frames reach the screen (Rust picks
   * the presenter: the native layer unless `KLIPOLO_PRESENTER=socket`). A page that attaches again starts paused.
   */
  async attach(onEvent: (event: EngineEventBinding) => void): Promise<EngineInfoBinding> {
    const events = new Channel<EngineEventBinding>();
    events.onmessage = onEvent;
    return call<EngineInfoBinding>('engine_attach', { events });
  },
  /** The format and device-pixel size to render. */
  setView: (view: ViewSpecBinding) => call<void>('engine_set_view', { view }),
  /** Where the native layer goes (the preview hole), or null to hide it. */
  setRect: (rect: ViewRectBinding | null) => call<void>('engine_view', { rect }),
  /** Hidden or minimised (false): the engine pauses with silence and lets its decoders go; shown again (true). */
  setVisible: (visible: boolean) => call<void>('engine_set_visible', { visible }),
  seek: (atUs: number, seq: number) => call<void>('engine_seek', { atUs, seq }),
  /**
   * Rust owns the clock and the sound; frames carry their time and `state` events report stops. `rate`: -2, -1, 1 or 2
   * (J/K/L shuttle; sound at 1x only). While playing it changes the speed or direction from `fromUs`. `seq` names the
   * run as a seek's does: its frames carry it, and a seek sent with an older one is ignored.
   */
  play: (fromUs: number, rate: number, seq: number) => call<void>('engine_play', { fromUs, rate, seq }),
  /** Resolves with where playback stopped. */
  pause: () => call<number>('engine_pause'),
  /** A dry run of `ops` on the open project at `baseRevision` (a drag, a slider): drawn at once, never committed. */
  preview: (projectId: string, baseRevision: number, ops: Op[]) =>
    call<void>('engine_preview', { projectId, baseRevision, ops }),
  previewClear: () => call<void>('engine_preview_clear'),
};
