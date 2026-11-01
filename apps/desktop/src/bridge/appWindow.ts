import { call } from './call';

/** The window's own commands (`crates/klipolo-app/src/commands/window.rs`). */
export const appWindow = {
  /** Puts the window in full screen (true) or takes it out; resolves with whether it was in full screen before. */
  fullScreen: (on: boolean) => call<boolean>('window_full_screen', { on }),
  /** Whether the window is in full screen now. */
  isFullScreen: () => call<boolean>('window_is_full_screen'),
};
