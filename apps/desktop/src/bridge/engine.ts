import { Channel } from '@tauri-apps/api/core';
import type { EngineEvent } from '../bindings/EngineEvent';
import type { EngineInfo } from '../bindings/EngineInfo';
import type { Op } from '../bindings/Op';
import type { ViewRect } from '../bindings/ViewRect';
import type { ViewSpec } from '../bindings/ViewSpec';
import { call } from './call';

export type { EngineEvent, EngineInfo, ViewRect, ViewSpec };

/**
 * The GPU preview engine (Rust owns it, `crates/klipolo-app/src/engine.rs`). Frames reach the screen through a native
 * layer under the webview (macOS), or over the frame socket (`frameSocket.ts`) into the page's canvas. Times are
 * integer microseconds, as in Rust.
 */
export const engine = {
  /**
   * Starts the engine if needed; the other calls do nothing until it runs. Says how frames reach the screen (Rust picks
   * the presenter: the native layer unless `KLIPOLO_PRESENTER=socket`). A page that attaches again starts paused.
   */
  async attach(onEvent: (event: EngineEvent) => void): Promise<EngineInfo> {
    const events = new Channel<EngineEvent>();
    events.onmessage = onEvent;
    return call<EngineInfo>('engine_attach', { events });
  },
  /** The format and device-pixel size to render. */
  setView: (view: ViewSpec) => call<void>('engine_set_view', { view }),
  /** Where the native layer goes (the preview hole), or null to hide it. */
  setRect: (rect: ViewRect | null) => call<void>('engine_view', { rect }),
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
  /**
   * The socket presenter's measured time (ms) from a playing frame being sent to it arriving here (recent median): the
   * engine renders frames that far ahead of the sound, so picture and sound line up.
   */
  presentDelay: (ms: number) => call<void>('engine_present_delay', { ms }),
  /** A dry run of `ops` on the open project at `baseRevision` (a drag, a slider): drawn at once, never committed. */
  preview: (projectId: string, baseRevision: number, ops: Op[]) =>
    call<void>('engine_preview', { projectId, baseRevision, ops }),
  previewClear: () => call<void>('engine_preview_clear'),
  /** While a title is typed on the canvas the page draws it: the preview leaves out text layer `item` (null: none). */
  setEditing: (item: string | null) => call<void>('engine_set_editing', { item }),
};
