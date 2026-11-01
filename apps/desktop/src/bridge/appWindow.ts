import { listen, TauriEvent, type UnlistenFn } from '@tauri-apps/api/event';
import { call } from './call';

/** The window's own commands (`crates/klipolo-app/src/commands/window.rs`) and its resize. */
export const appWindow = {
  /** Puts the window in full screen (true) or takes it out; resolves with whether it was in full screen before. */
  fullScreen: (on: boolean) => call<boolean>('window_full_screen', { on }),
  /** Whether the window is in full screen now. */
  isFullScreen: () => call<boolean>('window_is_full_screen'),
  /**
   * Calls `handler` each time the window resizes, as the window says it (Tauri's resize event): sent too once the
   * window has left full screen, which the page's own resize can come before. Resolves with the stop.
   */
  onResized: (handler: () => void): Promise<UnlistenFn> => listen(TauriEvent.WINDOW_RESIZED, () => handler()),
};
