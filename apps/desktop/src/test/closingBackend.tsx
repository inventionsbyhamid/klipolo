/* eslint-disable @typescript-eslint/no-explicit-any -- the mocked backend's IPC arguments */
import { act } from '@testing-library/react';
import type { ReactElement, ReactNode } from 'react';
import type { CaptionSettings } from '../bindings/CaptionSettings';
import type { Project } from '../bindings/Project';
import type { WordFix } from '../bindings/WordFix';
import { useProject } from '../state/project';
import { calls, captionsBackend, captionsSnapshot } from './captionsBackend';
import { renderWithProviders } from './render';
import { applying, wordLooks } from './wordPages';

/** What the app shows only while a project is open (its editor): gone once the close lands, as the editor goes. */
function WhileOpen({ children }: { children: ReactNode }) {
  const open = useProject((s) => s.snapshot !== null);
  return open ? children : null;
}

/** `ui` rendered as the editor holds it: only while a project is open. */
export const renderOpen = (ui: ReactElement) => renderWithProviders(<WhileOpen>{ui}</WhileOpen>);

/**
 * Rust as a close and a reopening see it: it keeps the caption settings, word looks and fixes it applies,
 * and each answer carries them, so the next change builds on them; it saves them as the project closes and reads them
 * back as it opens. An edit that comes once the project is closed finds no project. `extra` answers other commands
 * (the word list's pages); `wait`, when it gives a promise for a transaction, holds the answer until it settles.
 */
export function closingBackend(
  extra: Record<string, (args: any) => unknown> = {},
  wait: (tx: any) => Promise<void> | undefined = () => undefined,
) {
  const looks = applying();
  let open = true;
  let captions: CaptionSettings = useProject.getState().snapshot!.project.captions;
  let fixes: WordFix[] = [];
  captionsBackend({
    project_apply: async (args: any) => {
      if (!open) throw new Error('No project is open.');
      await wait(args.tx);
      for (const op of args.tx.ops) {
        if (op.type === 'setCaptions') captions = op.captions;
        if (op.type === 'fixWords')
          fixes = [...fixes, { assetId: op.assetId, startUs: op.startUs, endUs: op.endUs, text: op.text }];
      }
      const answer = looks(args);
      return { ...answer, patch: { ...answer.patch, captions, wordFixes: fixes } };
    },
    project_close: () => {
      open = false;
      return null;
    },
    project_open: () => {
      open = true;
      const snap = captionsSnapshot(true);
      snap.project = { ...snap.project, captions, wordLooks: [...wordLooks()], wordFixes: fixes };
      return snap;
    },
    ...extra,
  });
}

/** The edits and the close, in the order Rust heard them: each edit by its first op's type. */
export const heard = () =>
  calls
    .filter((c) => c.cmd === 'project_apply' || c.cmd === 'project_close')
    .map((c: any) => (c.cmd === 'project_close' ? 'close' : c.args.tx.ops[0].type));

/**
 * Closes the project (`landing`, if given, lands the commit the edits wait behind while the close waits for them; the
 * editor goes once it is closed, with `unmount`) and opens it again: the project as Rust saved it.
 */
export async function closeAndReopen({
  landing,
  unmount,
}: { landing?: () => void; unmount?: () => void } = {}): Promise<Project> {
  const id = useProject.getState().snapshot!.project.id;
  await act(async () => {
    const closed = useProject.getState().close();
    landing?.();
    await closed;
  });
  unmount?.();
  await act(() => useProject.getState().open(id));
  return useProject.getState().snapshot!.project;
}
