import type { UnlistenFn } from '@tauri-apps/api/event';
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
/** The window is in full screen while the preview fills it (put there, or there already): its leaving ends both. */
let windowFull = false;

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
      windowFull = true;
    } catch {
      // The window stays as it is: the preview still fills it.
      ours = false;
      windowFull = false;
    }
  },
  leave: async () => {
    if (!get().on) return;
    set({ on: false });
    windowFull = false;
    if (!ours) return;
    ours = false;
    await appWindow.fullScreen(false).catch(() => undefined);
  },
  toggle: () => (get().on ? get().leave() : get().enter()),
}));

/**
 * Keeps full screen honest: the window can leave full screen another way (the green button, ⌃⌘F), and then the preview
 * goes back into the editor too, whoever put the window there. Checked once a resize settles: the page's, and the
 * window's own, which it also sends once it has left (the page's last resize can come before that, while it still
 * reads as full screen). Closing the project leaves it. Returns the stop.
 */
export function followFullScreen(): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopped = false;
  let unlisten = null as UnlistenFn | null;
  const check = () => {
    if (!useFullScreen.getState().on || !windowFull) return;
    void appWindow.isFullScreen().then(
      (full) => {
        if (full || !useFullScreen.getState().on || !windowFull) return;
        // The window is out already: only the preview goes back.
        ours = false;
        windowFull = false;
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
  appWindow.onResized(resized).then(
    (stop) => {
      if (stopped) stop();
      else unlisten = stop;
    },
    // Without Tauri (a test page) the page's own resizes still count.
    () => undefined,
  );
  const off = useProject.subscribe((s) => {
    if (!s.snapshot && useFullScreen.getState().on) void useFullScreen.getState().leave();
  });
  return () => {
    stopped = true;
    clearTimeout(timer);
    window.removeEventListener('resize', resized);
    unlisten?.();
    off();
  };
}
