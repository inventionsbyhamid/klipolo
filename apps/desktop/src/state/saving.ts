import type { UnlistenFn } from '@tauri-apps/api/event';
import { create } from 'zustand';
import { bridge } from '../bridge/commands';
import { toast, useToasts } from './toasts';

/** Why saves keep failing (what the top bar and its notice say), or null while they work. */
export const useSaving = create<{ failing: string | null }>(() => ({ failing: null }));

const NOTICE = 'project.save';

/**
 * Follows the background saver (`project-save`): when saves start failing, a notice that stays says why and what to
 * do; when they work again, it goes and a note says so.
 */
export function followSaves(): Promise<UnlistenFn> {
  return bridge.onSaveStatus((status) => {
    if (status.ok) {
      useSaving.setState({ failing: null });
      useToasts.getState().dismissKey(NOTICE);
      toast.info('Your changes are saved again.', { done: true });
      return;
    }
    const failing = status.message ?? "Changes aren't being saved.";
    useSaving.setState({ failing });
    toast.error(failing, { key: NOTICE, persistent: true });
  });
}
