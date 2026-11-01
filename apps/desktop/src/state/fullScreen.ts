import { create } from 'zustand';
import { appWindow } from '../bridge/appWindow';
import { useProject } from './project';

/** How long the window's size must hold still before full screen is checked again (its own animation resizes it). */
export const FULL_SCREEN_SETTLE_MS = 500;

interface FullScreenStore {
  /** The preview fills the window, and the window the screen. */
  on: boolean;
  enter: () => Promise<void>;
  leave: () => Promise<void>;
  toggle: () => Promise<void>;
}

/** Klipolo put the window in full screen for the preview, so it takes it out again on leaving. */
let ours = false;

/**
 * Full screen preview (F; Escape leaves): the preview panel fills the window and the window goes full screen, so the
 * video fills the screen. A window the person had put in full screen already stays there on leaving. The page lays the
 * preview out again (`.app.is-full`): the frame box changes size, so the preview's size and the native layer's place
 * are sent again as they are for any other resize.
 */
export const useFullScreen = create<FullScreenStore>((set, get) => ({
  on: false,
  enter: async () => {
    if (get().on) return;
    set({ on: true });
    try {
      const was = await appWindow.fullScreen(true);
      // Left again while the window was asked: it goes back as it was.
      if (!get().on) {
        if (!was) await appWindow.fullScreen(false);
        return;
      }
      ours = !was;
    } catch {
      // The window stays as it is: the preview still fills it.
      ours = false;
    }
  },
  leave: async () => {
    if (!get().on) return;
    set({ on: false });
    if (!ours) return;
    ours = false;
    await appWindow.fullScreen(false).catch(() => undefined);
  },
  toggle: () => (get().on ? get().leave() : get().enter()),
}));

/**
 * Keeps full screen honest: the window can leave full screen another way (the green button, ⌃⌘F), and then the preview
 * goes back into the editor too; checked once a resize settles. Closing the project leaves it. Returns the stop.
 */
export function followFullScreen(): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const check = () => {
    if (!useFullScreen.getState().on || !ours) return;
    void appWindow.isFullScreen().then(
      (full) => {
        if (full || !useFullScreen.getState().on || !ours) return;
        // The window is out already: only the preview goes back.
        ours = false;
        useFullScreen.setState({ on: false });
      },
      () => undefined,
    );
  };
  const resized = () => {
    clearTimeout(timer);
    timer = setTimeout(check, FULL_SCREEN_SETTLE_MS);
  };
  window.addEventListener('resize', resized);
  const off = useProject.subscribe((s) => {
    if (!s.snapshot && useFullScreen.getState().on) void useFullScreen.getState().leave();
  });
  return () => {
    clearTimeout(timer);
    window.removeEventListener('resize', resized);
    off();
  };
}
