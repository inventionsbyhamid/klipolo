import { useEffect } from 'react';
import type { Project } from '../../bindings/Project';
import { findText, useTextEdit } from './textEdit';

/** What a key pressed in the canvas editor does. */
export type EditKey =
  /** The input method owns it (a composition is on): nothing of ours happens, Return and Escape included. */
  | 'composing'
  /** Escape, ⌘Return or Ctrl+Return: the edit finishes, keeping the words (open question 2). */
  | 'finish'
  /** Anything else types (plain Return starts a new line): it never reaches the editor's shortcuts. */
  | 'type';

interface KeyFacts {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  /** `isComposing`, or WebKit's keyCode 229 for the key that ends a composition. */
  composing: boolean;
}

export function editKey({ key, metaKey, ctrlKey, composing }: KeyFacts): EditKey {
  if (composing) return 'composing';
  if (key === 'Escape') return 'finish';
  if (key === 'Enter' && (metaKey || ctrlKey)) return 'finish';
  return 'type';
}

/** Elements that are part of the edit: the canvas editor and the sidebar field while it edits the same words. */
export const EDIT_PART = '[data-text-edit]';

/**
 * While a title is typed: a press anywhere outside the edit (another layer, the stage, the timeline, a button) finishes
 * it first, keeping the words; and a layer gone meanwhile (undone, or removed from chat) ends it, the words kept on the
 * clipboard when they had changed.
 */
export function useEditEnds(project: Project) {
  const active = useTextEdit((s) => s.session !== null);
  useEffect(() => {
    if (!active) return;
    const down = (e: PointerEvent) => {
      if (e.target instanceof Element && e.target.closest(EDIT_PART)) return;
      void useTextEdit.getState().finish();
    };
    document.addEventListener('pointerdown', down, true);
    return () => document.removeEventListener('pointerdown', down, true);
  }, [active]);
  const gone = useTextEdit(
    (s) => s.session !== null && s.session.draft === null && findText(project, s.session.id) === null,
  );
  useEffect(() => {
    if (gone) void useTextEdit.getState().finish();
  }, [gone]);
}
