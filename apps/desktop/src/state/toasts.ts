import { create } from 'zustand';

/**
 * A button on a toast: what it says (in the language shown) and what it does ("Save a copy…", "Unmute track"). The
 * toast closes as it runs.
 */
export interface ToastAction {
  label: string;
  run: () => void;
}

/** What a toast can carry besides its words. */
export interface ToastOptions {
  /** A newer toast with the same key replaces this one where it shows, so a notice said twice shows once. */
  key?: string;
  /** Its buttons, in order. */
  actions?: ToastAction[];
  /**
   * It stays until one of its actions is pressed or it is dismissed (a notice the person has to deal with, such as
   * "Not saved"); any other toast closes after a few seconds. Escape closes it only from inside it.
   */
  persistent?: boolean;
  /** Called once as it goes, however it goes: dismissed, timed out, an action pressed, or replaced by a newer one. */
  onClose?: () => void;
  /** A result the person asked for ("Added 2 clips"): shown with a check rather than the note's "i". */
  done?: boolean;
}

export interface ToastMessage extends ToastOptions {
  id: number;
  kind: 'error' | 'info';
  text: string;
}

/** Plain toasts kept at once; older ones make room. A toast with a key or actions is never pushed out (`pinned`). */
const PLAIN_KEPT = 3;

/** A notice that needs the person (it has a key or actions): only they, its time, or its replacement remove it. */
function pinned(t: ToastMessage): boolean {
  return t.key !== undefined || (t.actions?.length ?? 0) > 0;
}

interface ToastStore {
  toasts: ToastMessage[];
  push: (kind: ToastMessage['kind'], text: string, options?: ToastOptions) => void;
  dismiss: (id: number) => void;
  /** Removes the toast with this key, if any (its notice no longer holds). */
  dismissKey: (key: string) => void;
}

let nextId = 1;

export const useToasts = create<ToastStore>((set) => ({
  toasts: [],
  push: (kind, text, options) =>
    set((s) => {
      const next: ToastMessage = { id: nextId++, kind, text, ...options };
      // Said again: it takes the old one's place. A key shows at most once, so nothing else moves or makes room.
      const replaced = next.key === undefined ? undefined : s.toasts.find((t) => t.key === next.key);
      if (replaced) {
        queueMicrotask(() => replaced.onClose?.());
        return { toasts: s.toasts.map((t) => (t.key === next.key ? next : t)) };
      }
      // Only plain toasts make room, oldest first.
      const plain = s.toasts.filter((t) => !pinned(t)).length + (pinned(next) ? 0 : 1);
      let drop = Math.max(0, plain - PLAIN_KEPT);
      const kept = s.toasts.filter((t) => {
        if (pinned(t) || drop === 0) return true;
        drop -= 1;
        return false;
      });
      return { toasts: [...kept, next] };
    }),
  dismiss: (id) =>
    set((s) => {
      for (const t of s.toasts) if (t.id === id) queueMicrotask(() => t.onClose?.());
      return { toasts: s.toasts.filter((t) => t.id !== id) };
    }),
  dismissKey: (key) =>
    set((s) => {
      for (const t of s.toasts) if (t.key === key) queueMicrotask(() => t.onClose?.());
      return { toasts: s.toasts.filter((t) => t.key !== key) };
    }),
}));

export const toast = {
  error: (text: string, options?: ToastOptions) => useToasts.getState().push('error', text, options),
  info: (text: string, options?: ToastOptions) => useToasts.getState().push('info', text, options),
};
