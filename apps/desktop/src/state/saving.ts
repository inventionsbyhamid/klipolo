import type { UnlistenFn } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { Snapshot } from '../bindings/Snapshot';
import { bridge } from '../bridge/commands';
import { useProject } from './project';
import { toast, useToasts } from './toasts';

/** Which version of the project is on the disk: its opening and revision. */
export interface SavedVersion {
  opening: number;
  revision: number;
}

interface SavingStore {
  /** Why saves keep failing (what the top bar and its notice say), or null while they work. */
  failing: string | null;
  /** The newest version written, or null before any. */
  saved: SavedVersion | null;
}

export const useSaving = create<SavingStore>(() => ({ failing: null, saved: null }));

/** Whether the open project is on the disk as the page holds it, still being written, or failing to save. */
export type SaveState = 'saved' | 'saving' | 'failing';

export function saveStateOf(snapshot: Snapshot | null, saving: SavingStore): SaveState {
  if (saving.failing) return 'failing';
  const { saved } = saving;
  const current =
    saved !== null && snapshot?.opening === saved.opening && snapshot.history.revision === saved.revision;
  return snapshot === null || current ? 'saved' : 'saving';
}

/** The save state now, for code outside components. */
export function saveState(): SaveState {
  return saveStateOf(useProject.getState().snapshot, useSaving.getState());
}

const NOTICE = 'project.save';

/** The version a project's snapshot is: what is on the disk when it has just been opened or made. */
function versionOf(snapshot: Snapshot): SavedVersion | null {
  return snapshot.opening == null ? null : { opening: snapshot.opening, revision: snapshot.history.revision };
}

/**
 * Follows the background saver (`project-save`): each save that lands says which version it wrote, so the top bar
 * says "Saving…" until the page's own version is on the disk. When saves start failing, a notice that stays says why
 * and what to do; when they work again, it goes and a note says so. A project just opened or made is saved as it is.
 */
export function followSaves(): Promise<UnlistenFn> {
  useProject.subscribe((s, prev) => {
    const now = s.snapshot;
    if (!now || (now.opening === prev.snapshot?.opening && now.project.id === prev.snapshot?.project.id)) return;
    useSaving.setState({ saved: versionOf(now) });
  });
  return bridge.onSaveStatus((status) => {
    if (!status.ok) {
      const failing = status.message ?? "Changes aren't being saved.";
      useSaving.setState({ failing });
      toast.error(failing, { key: NOTICE, persistent: true });
      return;
    }
    if (useSaving.getState().failing) {
      useToasts.getState().dismissKey(NOTICE);
      toast.info('Your changes are saved again.', { done: true });
    }
    const { opening, revision } = status;
    const saved = opening != null && revision != null ? { opening, revision } : useSaving.getState().saved;
    useSaving.setState({ failing: null, saved });
  });
}
